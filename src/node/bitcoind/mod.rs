// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control a `bitcoind` process.
//!
//! [`BitcoinD`] starts `bitcoind` on the regtest network.
//! It gives access to the JSON-RPC client, process data, and test operations.
//!
//! ## Start a [`Node`]
//!
//! ```rust
//! use halfin::node::bitcoind::BitcoinD;
//!
//! // Start a node with the default configuration.
//! let node = BitcoinD::new().unwrap();
//!
//! // Mine blocks.
//! let _hashes = node.generate(10).unwrap();
//! assert_eq!(node.get_chain_tip().unwrap(), 10);
//! ```
//!
//! ## Select a data directory
//!
//! By default, each [`BitcoinD`] instance uses a temporary directory.
//! [`Drop`] removes this directory.
//! Set [`BitcoinDConf::staticdir`] to keep the data after the process stops.
//!
//! [`Node`]: crate::node::Node

/// Version-specific RPC client aliases for the bundled `bitcoind`.
mod client_versions;
/// Bundled `bitcoind` version metadata.
mod versions;

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::Address;
use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Denomination;
use corepc_client::bitcoin::FeeRate;
use corepc_client::bitcoin::Network;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v30::AddNodeCommand;
use corepc_client::client_sync::v30::Client;
use tracing::debug;

use crate::CONNECTION_INTERVAL;
use crate::CONNECTION_TIMEOUT;
use crate::DataDir;
use crate::Error;
use crate::IPV4_LOCALHOST;
use crate::SPAWN_ATTEMPTS;
use crate::SPAWN_INTERVAL;
use crate::find_conflicting_argument;
use crate::get_available_port;
use crate::init_data_dir;
use crate::node::Node;
use crate::node::NodeArgs;
use crate::node::NodeClientError;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::node::RPC_PASS;
use crate::node::RPC_USER;
use crate::node::validate_node_arguments;
use crate::node::write_rpc_cookie;
use crate::pipe_to_tracing;

/// Name of the wallet created (or loaded) inside every [`BitcoinD`] instance.
const BITCOIND_WALLET: &str = "wallet";

/// Return the path to the downloaded `bitcoind` binary.
///
/// At compile time, `build.rs` downloads and extracts the binary.
/// It stores the binary path in `HALFIN_BITCOIND_PATH`.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the compiled-in binary path does not exist.
pub fn get_bitcoind_path() -> Result<PathBuf, Error> {
    let bin_name = BitcoinD::get_bin_name().to_string();
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_BITCOIND_PATH").unwrap_or(""));

    // Add the `.exe` suffix on Windows
    #[cfg(target_os = "windows")]
    if bin_path.extension().is_none() {
        bin_path.set_extension("exe");
    }

    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

/// Arguments specific to Bitcoin Core.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct BitcoinDArgs {
    /// Fee rate used when fee estimation has insufficient data.
    pub fallback_fee_rate: FeeRate,
}

/// Configuration for a [`BitcoinD`] instance.
///
/// # Directory precedence
///
/// Set only `tmpdir` or `staticdir`.
/// If you set both fields, the function returns [`Error::BothDirsSpecified`].
///
/// | `tmpdir` | `staticdir` | Result |
/// |----------|-------------|--------|
/// | `None`   | `None`      | System temporary directory (deleted at `Drop`) |
/// | `Some`   | `None`      | Custom temporary root (deleted at `Drop`) |
/// | `None`   | `Some`      | Persistent directory (kept at `Drop`) |
/// | `Some`   | `Some`      | **Error** |
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BitcoinDConf {
    /// Arguments shared with other [`Node`] implementations.
    pub args: NodeArgs,

    /// Arguments specific to Bitcoin Core.
    pub bitcoind_args: BitcoinDArgs,

    /// Extra CLI arguments sent unchanged to the `bitcoind` process.
    ///
    /// Do not use a raw argument for an option in [`args`](Self::args) or
    /// [`bitcoind_args`](Self::bitcoind_args). A duplicate option returns
    /// [`NodeError::ConflictingArgument`].
    pub raw_args: Vec<String>,

    /// Root for the new temporary directory of each instance.
    /// If this field is empty, the function uses `TEMPDIR_ROOT`.
    /// If `TEMPDIR_ROOT` is empty, the function uses the system temporary directory.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory.
    /// The function creates the directory if necessary.
    /// [`Drop`] stops the process and keeps the files.
    pub staticdir: Option<PathBuf>,

    /// Maximum number of attempts to start `bitcoind`.
    ///
    /// Each attempt uses new random ports. Thus, a new attempt can correct a temporary port
    /// conflict. The default value is [`SPAWN_ATTEMPTS`].
    pub max_retries: u8,
}

impl Default for BitcoinDConf {
    fn default() -> Self {
        Self {
            args: NodeArgs {
                network: Network::Regtest,
                fixed_peers: Vec::new(),
                cbf_index: true,
                prune: PruneMode::Disabled,
                v2_transport: true,
                txindex: true,
            },
            bitcoind_args: BitcoinDArgs {
                fallback_fee_rate: FeeRate::from_sat_per_vb_u32(10),
            },
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

impl AsRef<NodeArgs> for BitcoinDConf {
    fn as_ref(&self) -> &NodeArgs {
        &self.args
    }
}

/// A running `bitcoind` [`Node`].
///
/// [`BitcoinD::from_bin`] and related functions start the [`Node`].
/// [`Drop`] stops the [`Node`] and deletes its temporary files.
///
/// # Wallet
///
/// At startup, the [`Node`] creates or loads a wallet named `"wallet"`.
/// All RPC helpers that require a wallet use this wallet.
///
/// # Networking
///
/// At startup, the operating system selects temporary RPC and P2P ports.
/// Use [`get_rpc_socket`](BitcoinD::get_rpc_socket) and
/// [`get_p2p_socket`](BitcoinD::get_p2p_socket) to get these ports.
#[derive(Debug)]
pub struct BitcoinD {
    /// Handle for the `bitcoind` child process.
    process: Child,
    /// Authenticated JSON-RPC client for the [`Node`] wallet.
    pub client: Client,
    /// Data directory of the [`Node`] and its cleanup state.
    working_directory: DataDir,
    /// Complete configuration used to start the [`Node`].
    config: BitcoinDConf,
    /// Path to the cookie file used for RPC authentication.
    cookie_file: PathBuf,
    /// Address of the JSON-RPC server.
    rpc_socket: SocketAddr,
    /// Address of the P2P listener.
    p2p_socket: SocketAddr,
}

#[rustfmt::skip]
impl Node for BitcoinD {
    type Config = BitcoinDConf;

    fn get_name() -> &'static str { versions::BITCOIND_NAME }

    fn get_bin_name() -> &'static str { versions::BITCOIND_BIN_NAME }

    fn get_config(&self) -> &BitcoinDConf { self.get_config() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_rpc_socket(&self) -> SocketAddr { self.get_rpc_socket() }

    fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> { self.generate(count) }

    fn get_p2p_socket(&self) -> SocketAddr { self.get_p2p_socket() }

    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> { self.has_peer(socket) }

    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> { self.add_peer(socket) }

    fn get_peer_count(&self) -> Result<u32, Error> { self.get_peer_count() }

    fn get_chain_tip(&self) -> Result<u32, Error> { self.get_chain_tip() }

    fn get_filter_tip(&self) -> Result<u32, Error> { self.get_filter_tip() }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> { self.get_block_hash(height) }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        Ok(self.client.call(method, args).map_err(NodeError::JsonRpc)?)
    }
}

impl BitcoinD {
    /// Start [`BitcoinD`] with the binary from [`get_bitcoind_path`].
    /// Use the default [`BitcoinDConf`].
    ///
    /// If the binary is not in `target/bin/`, `build.rs` downloads it from `bitcoincore.org`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the [`Node`].
    pub fn new() -> Result<Self, Error> {
        Self::from_bin(get_bitcoind_path()?)
    }

    /// Start [`BitcoinD`] with the binary from [`get_bitcoind_path`].
    /// Use the specified [`BitcoinDConf`].
    ///
    /// If the binary is not in `target/bin/`, `build.rs` downloads it from `bitcoincore.org`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the [`Node`].
    /// Returns an error if the configuration is not valid.
    pub fn new_with_conf(conf: &BitcoinDConf) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_bitcoind_path()?, conf)
    }

    /// Start the binary at [`Path`] with the default [`BitcoinDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if `bitcoind_bin` is not valid or the function cannot start the [`Node`].
    pub fn from_bin<P: AsRef<Path>>(bitcoind_bin: P) -> Result<Self, Error> {
        Self::from_bin_with_conf(bitcoind_bin, &BitcoinDConf::default())
    }

    /// Start the binary at [`Path`] with the specified [`BitcoinDConf`].
    /// The method uses at most [`BitcoinDConf::max_retries`] attempts.
    ///
    /// 1. Select new temporary RPC and P2P ports.
    /// 2. Write an RPC cookie in a new data directory.
    /// 3. Start `bitcoind` with these ports and RPC credentials.
    /// 4. Create or load the default wallet and create an RPC client.
    /// 5. Wait a maximum of 5 seconds for the [`Node`] to respond.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is not valid or the function cannot create the working
    /// directory. Returns an error if RPC setup fails or all start attempts fail.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        bitcoind_bin: P,
        conf: &BitcoinDConf,
    ) -> Result<Self, Error> {
        let configured_args = Self::configured_args(conf)?;

        // Validate the `bitcoind_bin` path
        let bitcoind_bin = bitcoind_bin.as_ref();
        // The path must be absolute
        if !bitcoind_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: bitcoind_bin.display().to_string(),
            });
        }
        // The path must be a file
        if !bitcoind_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: bitcoind_bin.display().to_string(),
            });
        }

        'spawn_attempt: for _ in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-bitcoind-",
            )?;
            let cookie_file = write_rpc_cookie(&working_directory.path())?;

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, rpc_port));
            let rpc_url = format!("http://{}", rpc_socket);

            let p2p_port = get_available_port();
            let p2p_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, p2p_port));

            let datadir_arg = format!("-datadir={}", working_directory.path().display());
            let rpc_arg = format!("-rpcport={}", rpc_port);
            let rpcbind_arg = format!("-rpcbind={IPV4_LOCALHOST}");
            let rpcuser_arg = format!("-rpcuser={RPC_USER}");
            let rpcpassword_arg = format!("-rpcpassword={RPC_PASS}");
            let p2p_arg = format!("-bind={}", p2p_socket);

            debug!(
                "Spawning {} [RPC_SOCKET={}, P2P_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                rpc_socket,
                p2p_socket,
                working_directory.path().display()
            );

            let mut process = Command::new(bitcoind_bin)
                .args(&configured_args)
                .args(&conf.raw_args)
                .arg(&datadir_arg)
                .arg(&rpc_arg)
                .arg(&rpcbind_arg)
                .arg(&rpcuser_arg)
                .arg(&rpcpassword_arg)
                .arg(&p2p_arg)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            // Add a small timeout to let `bitcoind` fail
            // and retry in the case of a port collision.
            sleep(SPAWN_INTERVAL);

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    debug!(
                        "{} exited immediately, retrying with fresh ports",
                        Self::get_name()
                    );
                    let _ = process.kill();
                    let _ = process.wait();
                    continue;
                }
                Ok(None) => {}
            }

            // Pipe the node's stdout/stderr into `tracing` so its logs are
            // visible alongside halfin's own. The reader threads exit on EOF
            // when the process dies.
            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "bitcoind");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "bitcoind");
            }

            // Wallet-specific RPC endpoints are prefixed with `/wallet`.
            let wallet_url = format!("{}/wallet/{}", rpc_url, BITCOIND_WALLET);

            // Create RPC authentication using the cookie file.
            let auth = Auth::CookieFile(cookie_file.clone());
            let client_base = Self::create_base_rpc_client(&rpc_url, &auth)?;

            // Create a new wallet or load an existing wallet
            // named `BITCOIND_WALLET` with a 5 second timeout.
            let deadline = Instant::now() + Duration::from_secs(5);
            let client = loop {
                if Instant::now() > deadline {
                    let _ = process.kill();
                    let _ = process.wait();
                    continue 'spawn_attempt;
                }
                if client_base.create_wallet(BITCOIND_WALLET).is_ok()
                    || client_base.load_wallet(BITCOIND_WALLET).is_ok()
                {
                    if let Ok(client) = Client::new_with_auth(&wallet_url, auth.clone()) {
                        break client;
                    }
                }
                sleep(Duration::from_millis(200));
            };

            if Self::wait_for_client(&client, Duration::from_secs(5)).is_err() {
                let _ = process.kill();
                let _ = process.wait();
                continue;
            }

            sleep(Duration::from_millis(200));

            debug!(
                "Started {} [PID={}, RPC_SOCKET={}, P2P_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                process.id(),
                rpc_socket,
                p2p_socket,
                working_directory.path().display()
            );

            return Ok(Self {
                process,
                client,
                working_directory,
                config: conf.clone(),
                cookie_file,
                rpc_socket,
                p2p_socket,
            });
        }

        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Send `stop` via RPC and wait for the process to exit.
    ///
    /// [`Drop`] stops the process without a call to this method.
    /// Call this method to get the exit status or confirm that the process has stopped.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC stop call fails.
    /// Returns an error if the function cannot wait for the child process.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());
        // Send a `stop` over RPC.
        let _ = self.client.stop().map_err(NodeError::FailedToStop)?;
        // Wait for the process to terminate and get its exit status.
        let exit_status = self.process.wait().map_err(Error::Io)?;

        Ok(exit_status)
    }

    /// Return the process ID of [`BitcoinD`].
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", Self::get_name(), pid);

        pid
    }

    /// Return the data directory of [`BitcoinD`].
    pub fn get_working_directory(&self) -> PathBuf {
        let working_directory = self.working_directory.path();

        debug!(
            "{}: got working directory at path={}",
            Self::get_name(),
            working_directory.display()
        );

        working_directory
    }

    /// Return the complete configuration used to start this [`Node`].
    pub fn get_config(&self) -> &BitcoinDConf {
        &self.config
    }

    /// Return the P2P [`SocketAddr`] of [`BitcoinD`].
    ///
    /// Pass this to [`BitcoinD::add_peer`] on another [`Node`] to connect the two.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        debug!(
            "{}: got p2p socket at socket={}",
            Self::get_name(),
            self.p2p_socket
        );

        self.p2p_socket
    }

    /// Return a reference to the RPC [`Client`] of [`BitcoinD`].
    pub fn get_rpc_client(&self) -> &Client {
        debug!(
            "{}: got rpc client for socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        &self.client
    }

    /// Return the JSON-RPC [`SocketAddr`] of [`BitcoinD`].
    pub fn get_rpc_socket(&self) -> SocketAddr {
        debug!(
            "{}: got rpc socket at socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        self.rpc_socket
    }

    /// Return the [`Path`] of the [`BitcoinD`] cookie file.
    pub fn get_cookie_file(&self) -> &Path {
        debug!(
            "{}: got cookie file at path={}",
            Self::get_name(),
            self.cookie_file.display()
        );

        &self.cookie_file
    }

    /// Return the current chain height.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let response = self
            .client
            .get_blockchain_info()
            .map_err(NodeError::JsonRpc)?;
        let height = response.blocks as u32;

        debug!("{}: got chain tip at height={}", Self::get_name(), height);

        Ok(height)
    }

    /// Return the current filter height.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails or the block filter index is unavailable.
    pub fn get_filter_tip(&self) -> Result<u32, Error> {
        let response = self.client.get_index_info().map_err(NodeError::JsonRpc)?;
        let filter_height = response
            .0
            .get("basic block filter index")
            .map(|i| i.best_block_height)
            .ok_or_else(|| {
                Error::UnexpectedResponse(
                    "BitcoinD does not have `blockfilterindex=1` enabled".to_string(),
                )
            })?;

        debug!(
            "{}: got filter tip at height={}",
            Self::get_name(),
            filter_height
        );

        Ok(filter_height)
    }

    /// Return the [`BlockHash`] of the block at `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails or the response is not a valid block hash.
    pub fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        let hash = self
            .client
            .get_block_hash(u64::from(height))
            .map_err(NodeError::JsonRpc)?
            .0
            .parse::<BlockHash>()
            .map_err(|e| Error::UnexpectedResponse(e.to_string()))?;

        debug!(
            "{}: got block hash at height={} hash={}",
            Self::get_name(),
            height,
            hash
        );

        Ok(hash)
    }

    /// Check whether this [`BitcoinD`] has a peer with the specified [`SocketAddr`].
    ///
    /// # Errors
    ///
    /// Returns an error if the peer-info JSON-RPC call fails.
    pub fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> {
        let peers = self.client.get_peer_info().map_err(NodeError::JsonRpc)?;
        let has_peer = peers
            .0
            .iter()
            .any(|p| p.address.contains(&socket.to_string()));

        debug!(
            "{}: checked peer connection at socket={} connected={}",
            Self::get_name(),
            socket,
            has_peer
        );

        Ok(has_peer)
    }

    /// Connect this [`BitcoinD`] to a peer at [`socket`](SocketAddr)
    /// and wait until the connection is established.
    ///
    /// # Errors
    ///
    /// Returns an error if the add-node RPC call fails or the peer does not
    /// appear in `getpeerinfo` within the timeout.
    pub fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> {
        debug!("{}: adding peer with socket={}", Self::get_name(), socket);

        self.client
            .add_node(&socket.to_string(), AddNodeCommand::Add)
            .map_err(NodeError::JsonRpc)?;

        let mut delay = CONNECTION_INTERVAL;

        let start = Instant::now();
        while start.elapsed() < CONNECTION_TIMEOUT {
            let peers = self.client.get_peer_info().map_err(NodeError::JsonRpc)?;
            if peers
                .0
                .iter()
                .any(|p| p.address.contains(&socket.to_string()))
            {
                debug!("{}: connected peer at socket={}", Self::get_name(), socket);
                return Ok(());
            }
            sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(NodeError::PeerConnectionTimeout((self.get_p2p_socket(), socket)).into())
    }

    /// Return the peer count of [`BitcoinD`].
    ///
    /// # Errors
    ///
    /// Returns an error if the peer-info JSON-RPC call fails.
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let peers = self.client.get_peer_info().map_err(NodeError::JsonRpc)?.0;
        let peer_count = peers.len() as u32;

        debug!("{}: got peer count value={}", Self::get_name(), peer_count);

        Ok(peer_count)
    }

    /// Generate `count` blocks.
    ///
    /// Returns the block hashes as a [`Vec<BlockHash>`].
    ///
    /// # Errors
    ///
    /// Returns an error if address generation, block generation, or block-hash parsing fails.
    pub fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> {
        debug!("{}: generating count={} block(s)", Self::get_name(), count);

        let address = self.client.new_address().map_err(NodeError::JsonRpc)?;
        let hashes = self
            .client
            .generate_to_address(count as usize, &address)
            .map_err(NodeError::JsonRpc)?
            .0
            .iter()
            .map(|h| {
                h.parse::<BlockHash>()
                    .map_err(|e| Error::UnexpectedResponse(e.to_string()))
            })
            .collect::<Result<Vec<BlockHash>, Error>>()?;
        Ok(hashes)
    }

    /// Generate `count` blocks.
    /// Use the specified [`Address`] as the coinbase output [`Address`].
    ///
    /// Returns the block hashes as a [`Vec<BlockHash>`].
    ///
    /// # Errors
    ///
    /// Returns an error if block generation or block-hash parsing fails.
    pub fn generate_to_address(
        &self,
        count: u32,
        address: &Address,
    ) -> Result<Vec<BlockHash>, Error> {
        debug!(
            "{}: generating count={} block(s) to address={}",
            Self::get_name(),
            count,
            address
        );

        let hashes = self
            .client
            .generate_to_address(count as usize, address)
            .map_err(NodeError::JsonRpc)?
            .0
            .iter()
            .map(|h| {
                h.parse::<BlockHash>()
                    .map_err(|e| Error::UnexpectedResponse(e.to_string()))
            })
            .collect::<Result<Vec<BlockHash>, Error>>()?;
        Ok(hashes)
    }

    /// Invalidate `count` [`Block`](corepc_client::bitcoin::Block)s in the [`BitcoinD`] chain.
    ///
    /// # Errors
    ///
    /// Returns an error if a JSON-RPC call fails or the function cannot parse a returned hash.
    pub fn invalidate_blocks(&self, count: u32) -> Result<(), Error> {
        debug!(
            "{}: invalidating count={} block(s)",
            Self::get_name(),
            count,
        );

        for _ in 0..count {
            let hash = self
                .client
                .get_best_block_hash()
                .map_err(NodeError::JsonRpc)?
                .block_hash()
                .map_err(|e| Error::UnexpectedResponse(e.to_string()))?;

            let height = self
                .client
                .get_blockchain_info()
                .map_err(NodeError::JsonRpc)?
                .blocks as u32;

            self.client
                .invalidate_block(hash)
                .map_err(NodeError::JsonRpc)?;
            debug!(
                "{}: invalidated block at height={} and hash={}",
                Self::get_name(),
                height,
                hash
            );
        }

        Ok(())
    }

    /// Validate typed and raw configuration and create daemon arguments.
    fn configured_args(conf: &BitcoinDConf) -> Result<Vec<String>, Error> {
        const OPTIONS: &[&str] = &[
            "bind",
            "blockfilterindex",
            "chain",
            "connect",
            "datadir",
            "fallbackfee",
            "listen",
            "port",
            "prune",
            "regtest",
            "rpcbind",
            "rpcpassword",
            "rpcport",
            "rpcuser",
            "signet",
            "testnet",
            "testnet4",
            "txindex",
            "v2transport",
        ];
        const BOOLEAN_OPTIONS: &[&str] = &[
            "blockfilterindex",
            "listen",
            "prune",
            "regtest",
            "signet",
            "testnet",
            "testnet4",
            "txindex",
            "v2transport",
        ];

        validate_node_arguments(&conf.args)?;
        if let Some(arg) = find_conflicting_argument(&conf.raw_args, OPTIONS, BOOLEAN_OPTIONS) {
            return Err(NodeError::ConflictingArgument(arg).into());
        }

        let prune = match conf.args.prune {
            PruneMode::Disabled => "0".to_string(),
            PruneMode::Manual => "1".to_string(),
            PruneMode::Automatic(target_mib) => target_mib.to_string(),
        };
        let fallback_fee_per_kvb = conf
            .bitcoind_args
            .fallback_fee_rate
            .fee_vb(1_000)
            .ok_or_else(|| {
                NodeError::InvalidConfiguration("fallback fee rate is too large".to_string())
            })?;
        let bool_value = |value: bool| if value { '1' } else { '0' };

        let mut args = vec![
            format!("-chain={}", conf.args.network.to_core_arg()),
            format!("-blockfilterindex={}", bool_value(conf.args.cbf_index)),
            format!("-prune={prune}"),
            format!("-v2transport={}", bool_value(conf.args.v2_transport)),
            format!("-txindex={}", bool_value(conf.args.txindex)),
            format!(
                "-fallbackfee={}",
                fallback_fee_per_kvb.display_in(Denomination::Bitcoin)
            ),
        ];
        args.extend(
            conf.args
                .fixed_peers
                .iter()
                .map(|peer| format!("-connect={peer}")),
        );

        Ok(args)
    }

    /// Try to create an RPC client without a wallet.
    /// Make a maximum of 10 attempts at intervals of 200 milliseconds.
    fn create_base_rpc_client(rpc_url: &str, auth: &Auth) -> Result<Client, Error> {
        for _ in 0..10 {
            if let Ok(client) = Client::new_with_auth(rpc_url, auth.clone()) {
                return Ok(client);
            }
            sleep(Duration::from_millis(200));
        }
        let client = Client::new_with_auth(rpc_url, auth.clone()).map_err(|source| {
            NodeError::UnresponsiveNode {
                node: Self::get_name(),
                source: NodeClientError::from(source),
            }
        })?;

        Ok(client)
    }

    /// Poll `getblockchaininfo` at intervals of 200 milliseconds until it succeeds.
    ///
    /// Returns `Err` if the [`Node`] is not responsive within `timeout`.
    fn wait_for_client(rpc_client: &Client, timeout: Duration) -> Result<(), Error> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if rpc_client.get_blockchain_info().is_ok() {
                return Ok(());
            }
            sleep(Duration::from_millis(200));
        }

        Err(Error::ClientSetupTimeout)
    }
}

impl Drop for BitcoinD {
    /// Send a stop request if the [`Node`] uses a persistent directory.
    /// Then, terminate the process.
    ///
    /// Ignore errors from `stop`, `kill`, and `wait` to prevent a panic in `Drop`.
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            Self::get_name(),
            self.process.id()
        );
        if let DataDir::Persistent(_) = self.working_directory {
            let _ = self.stop();
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[cfg(all(test, halfin_node))]
mod test;
