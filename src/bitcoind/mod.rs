// SPDX-License-Identifier: MIT OR Apache-2.0

//! # BitcoinD: spawn and interact with a `bitcoind` process
//!
//! A utility crate for spinning up `bitcoind` processes in
//! **regtest**, useful for integration testing Bitcoin applications.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use halfin::bitcoind::BitcoinD;
//!
//! // Start a node with default configuration.
//! let node = BitcoinD::new().unwrap();
//!
//! // Mine some blocks
//! let _hashes = node.generate(10).unwrap();
//! assert_eq!(node.get_chain_tip().unwrap(), 10);
//! ```
//!
//! ## Directory Handling
//!
//! By default each [`BitcoinD`] instance uses a temporary directory that is
//! cleaned up when the instance is dropped. Pass a `staticdir` in
//! [`BitcoinDConf`] to keep data between runs.

mod client_versions;
mod versions;

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::Address;
use corepc_client::bitcoin::BlockHash;
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
use crate::NODE_BUILDING_ATTEMPTS;
use crate::NODE_BUILDING_INTERVAL;
use crate::Node;
use crate::get_available_port;
use crate::pipe_to_tracing;

/// Name of the wallet created (or loaded) inside every [`BitcoinD`] instance.
const BITCOIND_WALLET: &str = "wallet";

/// Return the path to the downloaded `bitcoind` binary.
///
/// The path is resolved at compile time from the `HALFIN_BITCOIND_PATH`
/// environment variable, which is set by `build.rs` after downloading
/// and extracting the binary.
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

/// Configuration for a [`BitcoinD`] instance.
///
/// # Directory precedence
///
/// Exactly one of `tmpdir` / `staticdir` may be set at a time; setting both
/// returns [`Error::BothDirsSpecified`].
///
/// | `tmpdir` | `staticdir` | Result |
/// |----------|-------------|--------|
/// | `None`   | `None`      | System temp dir (auto-cleaned on drop) |
/// | `Some`   | `None`      | Custom temp root (auto-cleaned on drop) |
/// | `None`   | `Some`      | Persistent directory (not cleaned on drop) |
/// | `Some`   | `Some`      | **Error** |
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BitcoinDConf<'a> {
    /// Extra CLI arguments forwarded verbatim to the `bitcoind` process.
    ///
    /// The defaults (`-regtest`, `-fallbackfee=0.0001`, `-blockfilterindex=1`)
    /// are always present when using [`BitcoinDConf::default`].
    /// Replace or extend this vec to customise the node (e.g. add `-txindex=1`).
    pub args: Vec<&'a str>,

    /// Root directory under which a fresh temporary working directory is
    /// created for each instance. Falls back to the `TEMPDIR_ROOT`
    /// environment variable, then the system temp dir.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory. The directory is created if it does not
    /// exist. Data survives [`Drop`]: the process is stopped but files are
    /// kept so you can inspect or reuse them.
    pub staticdir: Option<PathBuf>,

    /// How many times to retry spawning `bitcoind` before giving up.
    ///
    /// Each attempt picks fresh random ports, so transient port-collision
    /// errors are automatically recovered from. Defaults to [`NODE_BUILDING_ATTEMPTS`].
    pub max_retries: u8,
}

impl Default for BitcoinDConf<'_> {
    fn default() -> Self {
        BitcoinDConf {
            args: vec!["-regtest", "-fallbackfee=0.0001", "-blockfilterindex=1"],
            tmpdir: None,
            staticdir: None,
            max_retries: NODE_BUILDING_ATTEMPTS,
        }
    }
}

/// A running `bitcoind` regtest node.
///
/// The node is started in [`BitcoinD::from_bin`] (or one of its siblings) and
/// stopped — and its temporary files removed — when this value is dropped.
///
/// # Wallet
///
/// A wallet named `"wallet"` is created (or loaded) automatically on startup.
/// All RPC helpers that require a wallet (`generate`, `new_address`, …) use
/// this wallet.
///
/// # Networking
///
/// Both the RPC and P2P ports are chosen from the OS's ephemeral range at
/// startup.  Use [`rpc_socket`](BitcoinD::rpc_socket) and
/// [`get_p2p_socket`](BitcoinD::get_p2p_socket) to discover them after
/// construction.
#[derive(Debug)]
pub struct BitcoinD {
    /// Handle to the spawned `bitcoind` child process.
    process: Child,
    /// Authenticated JSON-RPC client scoped to the node's wallet.
    pub client: Client,
    /// Owns (and optionally cleans up) the node's data directory.
    working_directory: DataDir,
    /// Path to the cookie file used for RPC authentication.
    cookie_file: PathBuf,
    /// Address the JSON-RPC server is bound to.
    rpc_socket: SocketAddr,
    /// Address the P2P listener is bound to.
    p2p_socket: SocketAddr,
}

#[rustfmt::skip]
impl Node for BitcoinD {
    fn get_name() -> &'static str { "BitcoinD" }

    fn get_bin_name() -> &'static str { "bitcoind_v_31_0" }

    fn get_p2p_socket(&self) -> SocketAddr { self.get_p2p_socket() }

    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> { self.has_peer(socket) }

    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> { self.add_peer(socket) }

    fn get_peer_count(&self) -> Result<u32, Error> { self.get_peer_count() }

    fn get_chain_tip(&self) -> Result<u32, Error> { self.get_chain_tip() }

    fn get_filter_tip(&self) -> Result<u32, Error> { self.get_filter_tip() }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> { self.get_block_hash(height) }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        self.client.call(method, args).map_err(Error::JsonRpc)
    }
}

impl BitcoinD {
    // ----> NODE

    /// Start a [`BitcoinD`] node using the binary located by [`get_bitcoind_path`], with the default [`BitcoinDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `bitcoincore.org` per `build.rs`.
    pub fn new() -> Result<BitcoinD, Error> {
        BitcoinD::from_bin(get_bitcoind_path()?)
    }

    /// Start a [`BitcoinD`] node using the binary located by [`get_bitcoind_path`], with a custom [`BitcoinDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `bitcoincore.org` per `build.rs`.
    pub fn new_with_conf(conf: &BitcoinDConf) -> Result<BitcoinD, Error> {
        BitcoinD::from_bin_with_conf(get_bitcoind_path()?, conf)
    }

    /// Create a [`BitcoinD`] instance running the binary at [`Path`] with the default [`BitcoinDConf`].
    pub fn from_bin<P: AsRef<Path>>(bitcoind_bin: P) -> Result<BitcoinD, Error> {
        BitcoinD::from_bin_with_conf(bitcoind_bin, &BitcoinDConf::default())
    }

    /// Create a [`BitcoinD`] instance running the binary at [`Path`] with a custom [`BitcoinDConf`].
    /// The method retries up to [`BitcoinDConf::max_retries`] times.  On each attempt it:
    ///
    /// 1. Picks fresh ephemeral RPC and P2P ports.
    /// 2. Spawns `bitcoind` with those ports and a fresh data directory.
    /// 3. Waits for the cookie file to appear (up to 5 seconds).
    /// 4. Creates or loads the default wallet and builds an RPC client.
    /// 5. Waits for the node to become responsive (up to 5 seconds).
    ///
    /// Returns an error if all attempts are exhausted.
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        bitcoind_bin: P,
        conf: &BitcoinDConf,
    ) -> Result<BitcoinD, Error> {
        // Validate the `bitcoind_bin` path
        let bitcoind_bin = bitcoind_bin.as_ref();
        // The path must be absolute
        if !bitcoind_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: BitcoinD::get_bin_name().to_string(),
                path: bitcoind_bin.display().to_string(),
            });
        }
        // The path must be a file
        if !bitcoind_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: BitcoinD::get_bin_name().to_string(),
                path: bitcoind_bin.display().to_string(),
            });
        }

        for _ in 0..conf.max_retries {
            let working_directory = Self::init_work_dir(conf)?;
            let cookie_file = working_directory
                .path()
                .join(Network::Regtest.to_string())
                .join(".cookie");

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, rpc_port));
            let rpc_url = format!("http://{}", rpc_socket);

            let p2p_port = get_available_port();
            let p2p_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, p2p_port));

            let datadir_arg = format!("-datadir={}", working_directory.path().display());
            let rpc_arg = format!("-rpcport={}", rpc_port);
            let p2p_arg = format!("-bind={}", p2p_socket);

            debug!(
                "Spawning {} [RPC_SOCKET={}, P2P_SOCKET={}, DATADIR={}]",
                BitcoinD::get_name(),
                rpc_socket,
                p2p_socket,
                working_directory.path().display()
            );

            let mut process = Command::new(bitcoind_bin)
                .args(&conf.args)
                .arg(&datadir_arg)
                .arg(&rpc_arg)
                .arg(&p2p_arg)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            // Add a small timeout to let `bitcoind` fail
            // and retry in the case of a port collision.
            thread::sleep(NODE_BUILDING_INTERVAL);

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    debug!(
                        "{} exited immediately, retrying with fresh ports",
                        BitcoinD::get_name()
                    );
                    let _ = process.kill();
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

            // Wait up to 5 seconds for the cookie file. Kills
            // the process and tries again if it exceeds this time.
            if Self::wait_for_cookie_file(&cookie_file, Duration::from_secs(5)).is_err() {
                let _ = process.kill();
                continue;
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
                    continue;
                }
                if client_base.create_wallet(BITCOIND_WALLET).is_ok()
                    || client_base.load_wallet(BITCOIND_WALLET).is_ok()
                {
                    if let Ok(client) = Client::new_with_auth(&wallet_url, auth.clone()) {
                        break client;
                    }
                }
                thread::sleep(Duration::from_millis(200));
            };

            if Self::wait_for_client(&client, Duration::from_secs(5)).is_err() {
                let _ = process.kill();
                continue;
            }

            sleep(Duration::from_millis(200));

            debug!(
                "Started {} [PID={}, RPC_SOCKET={}, P2P_SOCKET={}, DATADIR={}]",
                BitcoinD::get_name(),
                process.id(),
                rpc_socket,
                p2p_socket,
                working_directory.path().display()
            );

            return Ok(BitcoinD {
                process,
                client,
                working_directory,
                cookie_file,
                rpc_socket,
                p2p_socket,
            });
        }

        Err(Error::ExhaustedNodeBuildingAttempts(conf.max_retries))
    }

    /// Send `stop` via RPC and wait for the process to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically.  It is provided for cases where you
    /// need the exit status or want to ensure the node has fully shut down
    /// before proceeding.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        debug!(
            "Stopping {} [PID={}]",
            BitcoinD::get_name(),
            self.process.id()
        );
        // Send a `stop` over RPC.
        let _ = self.client.stop().map_err(Error::FailedToStop)?;
        // Wait for the process to terminate and get its exit status.
        let exit_status = self.process.wait().map_err(Error::Io)?;

        Ok(exit_status)
    }

    /// Get [`BitcoinD`]'s PID process.
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", BitcoinD::get_name(), pid);

        pid
    }

    /// Get [`BitcoinD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        let working_directory = self.working_directory.path();

        debug!(
            "{}: got working directory at path={}",
            BitcoinD::get_name(),
            working_directory.display()
        );

        working_directory
    }

    /// Get [`BitcoinD`]'s P2P [`SocketAddr`].
    ///
    /// Pass this to [`BitcoinD::add_peer`] on another node to connect the two.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        debug!(
            "{}: got p2p socket at socket={}",
            BitcoinD::get_name(),
            self.p2p_socket
        );

        self.p2p_socket
    }

    /// Get a reference to [`BitcoinD`]'s RPC [`Client`].
    pub fn get_rpc_client(&self) -> &Client {
        debug!(
            "{}: got rpc client for socket={}",
            BitcoinD::get_name(),
            self.rpc_socket
        );

        &self.client
    }

    /// Get [`BitcoinD`]'s JSON-RPC [`SocketAddr`].
    pub fn rpc_socket(&self) -> SocketAddr {
        debug!(
            "{}: got rpc socket at socket={}",
            BitcoinD::get_name(),
            self.rpc_socket
        );

        self.rpc_socket
    }

    /// Get the [`Path`] to [`BitcoinD`]'s cookie file.
    pub fn cookie_file(&self) -> &Path {
        debug!(
            "{}: got cookie file at path={}",
            BitcoinD::get_name(),
            self.cookie_file.display()
        );

        &self.cookie_file
    }

    // ----> RPC CALL WRAPPERS

    /// Get the current chain height.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let response = self.client.get_blockchain_info().map_err(Error::JsonRpc)?;
        let height = response.blocks as u32;

        debug!(
            "{}: got chain tip at height={}",
            BitcoinD::get_name(),
            height
        );

        Ok(height)
    }

    /// Get the current filter height.
    pub fn get_filter_tip(&self) -> Result<u32, Error> {
        let response = self.client.get_index_info().map_err(Error::JsonRpc)?;
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
            BitcoinD::get_name(),
            filter_height
        );

        Ok(filter_height)
    }

    /// Get the [`BlockHash`] of the block at height `height`.
    pub fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        let hash = self
            .client
            .get_block_hash(height as u64)
            .map_err(Error::JsonRpc)?
            .0
            .parse::<BlockHash>()
            .map_err(|e| Error::UnexpectedResponse(e.to_string()))?;

        debug!(
            "{}: got block hash at height={} hash={}",
            BitcoinD::get_name(),
            height,
            hash
        );

        Ok(hash)
    }

    /// Check whether this [`BitcoinD`] has a peer with a specific [`SocketAddr`].
    pub fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> {
        let peers = self.client.get_peer_info().map_err(Error::JsonRpc)?;
        let has_peer = peers
            .0
            .iter()
            .any(|p| p.address.contains(&socket.to_string()));

        debug!(
            "{}: checked peer connection at socket={} connected={}",
            BitcoinD::get_name(),
            socket,
            has_peer
        );

        Ok(has_peer)
    }

    /// Connect this [`BitcoinD`] to a peer at [`socket`](SocketAddr)
    /// and wait until the connection is established.
    ///
    /// Returns an error if the peer does not appear in `getpeerinfo` within the timeout.
    pub fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> {
        debug!(
            "{}: adding peer with socket={}",
            BitcoinD::get_name(),
            socket
        );

        self.client
            .add_node(&socket.to_string(), AddNodeCommand::Add)
            .map_err(Error::JsonRpc)?;

        let mut delay = CONNECTION_INTERVAL;

        let start = Instant::now();
        while start.elapsed() < CONNECTION_TIMEOUT {
            let peers = self.client.get_peer_info().map_err(Error::JsonRpc)?;
            if peers
                .0
                .iter()
                .any(|p| p.address.contains(&socket.to_string()))
            {
                debug!(
                    "{}: connected peer at socket={}",
                    BitcoinD::get_name(),
                    socket
                );
                return Ok(());
            }
            thread::sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(Error::PeerConnectionTimeout((
            self.get_p2p_socket(),
            socket,
        )))
    }

    /// Get [`BitcoinD`]'s peer count.
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let peers = self.client.get_peer_info().map_err(Error::JsonRpc)?.0;
        let peer_count = peers.len() as u32;

        debug!(
            "{}: got peer count value={}",
            BitcoinD::get_name(),
            peer_count
        );

        Ok(peer_count)
    }

    /// Generate `count` blocks.
    ///
    /// Returns the block hashes as a [`Vec<BlockHash>`].
    pub fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> {
        debug!(
            "{}: generating count={} block(s)",
            BitcoinD::get_name(),
            count
        );

        let address = self.client.new_address().map_err(Error::JsonRpc)?;
        let hashes = self
            .client
            .generate_to_address(count as usize, &address)
            .map_err(Error::JsonRpc)?
            .0
            .iter()
            .map(|h| {
                h.parse::<BlockHash>()
                    .map_err(|e| Error::UnexpectedResponse(e.to_string()))
            })
            .collect::<Result<Vec<BlockHash>, Error>>()?;
        Ok(hashes)
    }

    /// Generate `count` blocks using the provided
    /// [`Address`] as the coinbase output [`Address`].
    ///
    /// Returns the block hashes as a [`Vec<BlockHash>`].
    pub fn generate_to_address(
        &self,
        count: u32,
        address: &Address,
    ) -> Result<Vec<BlockHash>, Error> {
        debug!(
            "{}: generating count={} block(s) to address={}",
            BitcoinD::get_name(),
            count,
            address
        );

        let hashes = self
            .client
            .generate_to_address(count as usize, address)
            .map_err(Error::JsonRpc)?
            .0
            .iter()
            .map(|h| {
                h.parse::<BlockHash>()
                    .map_err(|e| Error::UnexpectedResponse(e.to_string()))
            })
            .collect::<Result<Vec<BlockHash>, Error>>()?;
        Ok(hashes)
    }

    /// Invalidates `count` [`Block`](corepc_client::bitcoin::Block)s from [`BitcoinD`]'s chain.
    pub fn invalidate_blocks(&self, count: u32) -> Result<(), Error> {
        debug!(
            "{}: invalidating count={} block(s)",
            BitcoinD::get_name(),
            count,
        );

        for _ in 0..count {
            let hash = self
                .client
                .get_best_block_hash()
                .unwrap()
                .block_hash()
                .unwrap();

            let height = self
                .client
                .get_blockchain_info()
                .map_err(Error::JsonRpc)?
                .blocks as u32;

            self.client.invalidate_block(hash).unwrap();
            debug!(
                "{}: invalidated block at height={} and hash={}",
                BitcoinD::get_name(),
                height,
                hash
            );
        }

        Ok(())
    }

    // ----> INTERNAL

    /// Resolve and create the working directory according to `conf`.
    ///
    /// Precedence: `conf.tmpdir` → `TEMPDIR_ROOT` env var → system temp.
    /// If `conf.staticdir` is set the directory is created but never cleaned
    /// up automatically.
    fn init_work_dir(conf: &BitcoinDConf) -> Result<DataDir, Error> {
        let tmpdir = conf
            .tmpdir
            .clone()
            .or_else(|| env::var("TEMPDIR_ROOT").map(PathBuf::from).ok());
        let work_dir = match (&tmpdir, &conf.staticdir) {
            // Cannot specify both directories.
            (Some(_), Some(_)) => return Err(Error::BothDirsSpecified),
            // Create a persistent directory.
            (None, Some(workdir)) => {
                fs::create_dir_all(workdir).map_err(Error::Io)?;
                DataDir::Persistent(workdir.to_owned())
            }
            // Create a new temporary directory.
            (Some(tmpdir), None) => DataDir::Temporary(
                tempfile::Builder::new()
                    .prefix("halfin-bitcoind-")
                    .tempdir_in(tmpdir)
                    .map_err(Error::Io)?,
            ),
            (None, None) => DataDir::Temporary(
                tempfile::Builder::new()
                    .prefix("halfin-bitcoind-")
                    .tempdir()
                    .map_err(Error::Io)?,
            ),
        };
        Ok(work_dir)
    }

    /// Attempt to create a base (wallet-less) RPC client, retrying up to 10
    /// times with 200 millisecond gaps. Used during startup before the wallet exists.
    fn create_base_rpc_client(rpc_url: &str, auth: &Auth) -> Result<Client, Error> {
        for _ in 0..10 {
            if let Ok(client) = Client::new_with_auth(rpc_url, auth.clone()) {
                return Ok(client);
            }
            thread::sleep(Duration::from_millis(200));
        }
        let client =
            Client::new_with_auth(rpc_url, auth.clone()).map_err(Error::UnresponsiveBitcoinD)?;

        Ok(client)
    }

    /// Poll for the cookie file's existence, sleeping 200 milliseconds between checks.
    ///
    /// Returns `Err` if the file does not appear within `timeout`.
    fn wait_for_cookie_file(cookie_file: &Path, timeout: Duration) -> Result<(), Error> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cookie_file.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(Error::CookieFileTimeout(cookie_file.into()))
    }

    /// Poll `getblockchaininfo` until it succeeds, sleeping 200 milliseconds between attempts.
    ///
    /// Returns `Err` if the node is not responsive within `timeout`.
    fn wait_for_client(rpc_client: &Client, timeout: Duration) -> Result<(), Error> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if rpc_client.get_blockchain_info().is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }

        Err(Error::RpcClientSetupTimeout)
    }
}

impl Drop for BitcoinD {
    /// Gracefully stops the node (if it was started with a persistent
    /// directory) and kills the process.
    ///
    /// Errors from `stop` and `kill` are silently discarded so that `Drop`
    /// never panics.
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            BitcoinD::get_name(),
            self.process.id()
        );
        if let DataDir::Persistent(_) = self.working_directory {
            let _ = self.stop();
        }
        let _ = self.process.kill();
    }
}
