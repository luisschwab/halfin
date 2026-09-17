// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control a `libbitcoin-server` process.
//!
//! [`LibbitcoinD`] implements both [`Node`] and [`Indexer`]. It provides
//! Bitcoin Core JSON-RPC, Electrum, and Esplora HTTP endpoints.
//!
//! This version of `bs` uses mainnet settings. The wrapper does not support other networks.
//! You cannot use this server as the backing node for a separate [`Indexer`].
//!
//! # Start a [`LibbitcoinD`] process
//!
//! ```rust,no_run
//! use halfin::node::libbitcoind::LibbitcoinD;
//! use halfin::node::libbitcoind::LibbitcoinDConf;
//!
//! // Start with the default configuration.
//! let default_node = LibbitcoinD::new().unwrap();
//!
//! // Start with a custom configuration.
//! let conf = LibbitcoinDConf::default();
//! let custom_node = LibbitcoinD::new_with_conf(&conf).unwrap();
//!
//! // Use the node interface.
//! let height = default_node.get_chain_tip().unwrap();
//! let rpc_socket = custom_node.get_rpc_socket();
//! ```
//!
//! [`Node`]: crate::node::Node
//! [`Indexer`]: crate::indexer::Indexer

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::Txid;
use corepc_client::bitcoin::hashes::hex::HexToArrayError;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v30::Client;
use electrum_client::ElectrumApi;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use esplora_client::blocking::BlockingClient;
use serde_json::Value;
use tracing::debug;

use crate::DataDir;
use crate::Error;
use crate::INDEXING_TIMEOUT;
use crate::IPV4_LOCALHOST;
use crate::POLL_INTERVAL;
use crate::SPAWN_ATTEMPTS;
use crate::SPAWN_INTERVAL;
use crate::STARTUP_TIMEOUT;
use crate::find_conflicting_argument;
use crate::get_available_port;
use crate::indexer::Indexer;
use crate::indexer::IndexerError;
use crate::init_data_dir;
use crate::node::Node;
use crate::node::NodeArgs;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::node::RPC_PASS;
use crate::node::RPC_USER;
use crate::node::write_rpc_cookie;
use crate::pipe_to_tracing;

#[cfg(test)]
mod test;

/// Version of `libbitcoin-server` that this crate uses.
mod versions;

/// Return the path to the downloaded `bs` executable.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the executable does not exist.
pub fn get_libbitcoin_path() -> Result<PathBuf, Error> {
    let path = PathBuf::from(option_env!("HALFIN_LIBBITCOIN_PATH").unwrap_or(""));
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::BinaryNotFound((
            <LibbitcoinD as Node>::get_bin_name().to_string(),
            path,
        )))
    }
}

/// Settings for a [`LibbitcoinD`] process.
///
/// Set `tmpdir` or `staticdir`. Do not set both.
/// `tmpdir` selects the parent of a temporary data directory.
/// The wrapper deletes that data directory when it drops the process.
/// `staticdir` selects a data directory that the wrapper does not delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibbitcoinDConf {
    /// Settings that all [`Node`] implementations use.
    pub args: NodeArgs,
    /// Additional `bs` command-line arguments that the wrapper does not set.
    pub raw_args: Vec<String>,
    /// Parent directory for a temporary data directory.
    pub tmpdir: Option<PathBuf>,
    /// Data directory that remains after the process stops.
    pub staticdir: Option<PathBuf>,
    /// Maximum number of attempts to start the process.
    pub max_retries: u8,
}

impl Default for LibbitcoinDConf {
    fn default() -> Self {
        Self {
            args: NodeArgs {
                network: Network::Bitcoin,
                fixed_peers: Vec::new(),
                v2_transport: false,
                cbf_index: false,
                prune: PruneMode::Disabled,
                txindex: true,
            },
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

impl AsRef<NodeArgs> for LibbitcoinDConf {
    fn as_ref(&self) -> &NodeArgs {
        &self.args
    }
}

/// A running `libbitcoin-server` process with JSON-RPC, Electrum, and Esplora endpoints.
#[derive(Debug)]
pub struct LibbitcoinD {
    /// Child process that runs `bs`.
    process: Child,
    /// Client for the Bitcoin Core compatible RPC endpoint.
    pub client: Client,
    /// Client for the Electrum endpoint.
    pub electrum_client: RawClient<ElectrumPlaintextStream>,
    /// Blocking client for the Esplora HTTP endpoint.
    pub esplora_client: BlockingClient,
    /// Data directory and its deletion policy.
    working_directory: DataDir,
    /// Settings that started the process.
    config: LibbitcoinDConf,
    /// P2P listener.
    p2p_socket: SocketAddr,
    /// JSON-RPC listener.
    rpc_socket: SocketAddr,
    /// Electrum listener.
    electrum_socket: SocketAddr,
    /// Esplora HTTP listener.
    esplora_socket: SocketAddr,
}

#[rustfmt::skip]
impl Node for LibbitcoinD {
    type Config = LibbitcoinDConf;

    fn get_name() -> &'static str { versions::LIBBITCOIN_NAME }

    fn get_bin_name() -> &'static str { versions::LIBBITCOIN_BIN_NAME }

    fn get_config(&self) -> &Self::Config { self.get_config() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_rpc_socket(&self) -> SocketAddr { self.get_rpc_socket() }

    fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> { self.generate(count) }

    fn get_chain_tip(&self) -> Result<u32, Error> { self.get_chain_tip() }

    fn get_filter_tip(&self) -> Result<u32, Error> { self.get_filter_tip() }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> { self.get_block_hash(height) }

    fn call(&self, method: &str, args: &[Value]) -> Result<Value, Error> { self.call(method, args) }

    fn get_p2p_socket(&self) -> SocketAddr { self.get_p2p_socket() }

    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> { self.has_peer(socket) }

    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> { self.add_peer(socket) }

    fn get_peer_count(&self) -> Result<u32, Error> { self.get_peer_count() }
}

#[rustfmt::skip]
impl Indexer for LibbitcoinD {
    type Config = LibbitcoinDConf;

    fn get_name() -> &'static str { versions::LIBBITCOIN_NAME }

    fn get_bin_name() -> &'static str { versions::LIBBITCOIN_BIN_NAME }

    fn trigger(&self) -> Result<(), Error> { self.trigger() }

    fn stop(&mut self) -> Result<ExitStatus, Error> { self.stop() }

    fn get_pid(&self) -> u32 { self.get_pid() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_config(&self) -> &Self::Config { self.get_config() }

    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> { self.get_electrum_client() }

    fn get_electrum_socket(&self) -> SocketAddr { self.get_electrum_socket() }

    fn get_electrum_url(&self) -> String { self.get_electrum_url() }

    fn wait_until_caught_up(&self, node: &impl Node, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_caught_up(node, timeout) }

    fn wait_until_tip(&self, exp_height: u32, exp_hash: BlockHash, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_tip(exp_height, exp_hash, timeout) }

    fn wait_until_mempool_tx(&self, spk: &Script, txid: Txid, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_mempool_tx(spk, txid, timeout) }
}

impl LibbitcoinD {
    /// Return the name of this implementation.
    pub fn get_name() -> &'static str {
        versions::LIBBITCOIN_NAME
    }

    /// Return the name of the `bs` executable.
    pub fn get_bin_name() -> &'static str {
        versions::LIBBITCOIN_BIN_NAME
    }

    /// Start the downloaded `bs` executable with the default settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the executable does not exist or the process does not start.
    pub fn new() -> Result<Self, Error> {
        Self::from_bin(get_libbitcoin_path()?)
    }

    /// Start the downloaded `bs` executable with `conf`.
    ///
    /// # Errors
    ///
    /// Returns an error if `conf` is invalid, the executable does not exist,
    /// or the process does not start.
    pub fn new_with_conf(conf: &LibbitcoinDConf) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_libbitcoin_path()?, conf)
    }

    /// Start the `bs` executable at `bin` with the default settings.
    ///
    /// # Errors
    ///
    /// Returns an error if `bin` is invalid or the process does not start.
    pub fn from_bin<P: AsRef<Path>>(bin: P) -> Result<Self, Error> {
        Self::from_bin_with_conf(bin, &LibbitcoinDConf::default())
    }

    /// Start the `bs` executable at `bin` with `conf`.
    /// The wrapper gives the process a data directory and local network ports.
    ///
    /// # Errors
    ///
    /// Returns an error if `bin` or `conf` is invalid, or if the process does not start.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        bin: P,
        conf: &LibbitcoinDConf,
    ) -> Result<Self, Error> {
        Self::validate_conf(conf)?;
        let bin = bin.as_ref();
        if !bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: bin.display().to_string(),
            });
        }
        if !bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: bin.display().to_string(),
            });
        }

        for _ in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-libbitcoin-",
            )?;
            let p2p_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port()));
            let rpc_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port()));
            let electrum_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port()));
            let esplora_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port()));
            let config_path = working_directory.path().join("bs.cfg");
            let cookie_path = write_rpc_cookie(&working_directory.path())?;
            Self::write_config(
                &config_path,
                &working_directory.path(),
                p2p_socket,
                rpc_socket,
                electrum_socket,
                esplora_socket,
            )?;

            debug!(
                "Spawning LibbitcoinD [P2P={p2p_socket}, RPC={rpc_socket}, ELECTRUM={electrum_socket}, ESPLORA={esplora_socket}]"
            );
            let mut process = Command::new(bin)
                .arg("--config")
                .arg(&config_path)
                .args(&conf.raw_args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;
            sleep(SPAWN_INTERVAL);
            match process.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    let output = process.wait_with_output().map_err(Error::Io)?;
                    eprintln!(
                        "LibbitcoinD exited immediately with {status}; stdout={}; stderr={}",
                        String::from_utf8_lossy(&output.stdout).trim(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                    continue;
                }
                Err(_) => {
                    let _ = process.kill();
                    let _ = process.wait();
                    continue;
                }
            }
            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "libbitcoin");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "libbitcoin");
            }

            let rpc_url = format!("http://{rpc_socket}");
            let auth = Auth::CookieFile(cookie_path);
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            while Instant::now() < deadline {
                if process.try_wait().ok().flatten().is_some() {
                    break;
                }
                let Ok(client) = Client::new_with_auth(&rpc_url, auth.clone()) else {
                    sleep(Duration::from_millis(200));
                    continue;
                };
                if client.call::<Value>("getblockcount", &[]).is_ok() {
                    if let Ok(electrum_client) =
                        RawClient::new(electrum_socket, Some(Duration::from_millis(500)), None)
                    {
                        if electrum_client.ping().is_ok() {
                            let esplora_client =
                                esplora_client::Builder::new(&format!("http://{esplora_socket}"))
                                    .timeout(Duration::from_secs(10))
                                    .build_blocking();
                            if esplora_client.get_height().is_err() {
                                sleep(Duration::from_millis(200));
                                continue;
                            }
                            return Ok(Self {
                                process,
                                client,
                                electrum_client,
                                esplora_client,
                                working_directory,
                                config: conf.clone(),
                                p2p_socket,
                                rpc_socket,
                                electrum_socket,
                                esplora_socket,
                            });
                        }
                    }
                }
                sleep(Duration::from_millis(200));
            }
            let _ = process.kill();
            let _ = process.wait();
        }
        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Send the console stop command to `bs`, then wait for the process to exit.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the wrapper cannot write the command or wait for the process.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        if let Some(stdin) = self.process.stdin.as_mut() {
            stdin.write_all(b"c\n").map_err(Error::Io)?;
            stdin.flush().map_err(Error::Io)?;
        }
        self.process.wait().map_err(Error::Io)
    }

    /// Return the process ID.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }

    /// Return the data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }

    /// Return the settings that started the process.
    pub fn get_config(&self) -> &LibbitcoinDConf {
        &self.config
    }

    /// Return the JSON-RPC socket.
    pub fn get_rpc_socket(&self) -> SocketAddr {
        self.rpc_socket
    }

    /// Return the Electrum socket.
    pub fn get_electrum_socket(&self) -> SocketAddr {
        self.electrum_socket
    }

    /// Return the Electrum URL.
    pub fn get_electrum_url(&self) -> String {
        format!("tcp://{}", self.electrum_socket)
    }

    /// Return the Electrum client.
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        &self.electrum_client
    }

    /// Return the Esplora HTTP client.
    pub fn get_esplora_client(&self) -> &BlockingClient {
        &self.esplora_client
    }

    /// Return the Esplora HTTP listener.
    pub fn get_esplora_socket(&self) -> SocketAddr {
        self.esplora_socket
    }

    /// Return the Esplora HTTP URL.
    pub fn get_esplora_url(&self) -> String {
        format!("http://{}", self.esplora_socket)
    }

    /// Return the P2P socket.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        self.p2p_socket
    }

    /// Report that this wrapper does not support block generation.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::UnsupportedCommand`].
    pub fn generate(&self, _count: u32) -> Result<Vec<BlockHash>, Error> {
        Err(Self::unsupported_node("generate"))
    }

    /// Return the current block height from the JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call fails or does not return a `u32` height.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let value = self.call("getblockcount", &[])?;
        value
            .as_u64()
            .and_then(|height| u32::try_from(height).ok())
            .ok_or_else(|| {
                Error::UnexpectedResponse("getblockcount returned a non-u32 height".to_string())
            })
    }

    /// Report that this wrapper does not provide a compact filter height.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::UnsupportedCommand`].
    pub fn get_filter_tip(&self) -> Result<u32, Error> {
        Err(Self::unsupported_node("get_filter_tip"))
    }

    /// Return the block hash at `height` from the JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call fails or does not return a block hash.
    pub fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        let value = self.call("getblockhash", &[height.into()])?;
        value
            .as_str()
            .ok_or_else(|| {
                Error::UnexpectedResponse("getblockhash returned a non-string hash".to_string())
            })?
            .parse()
            .map_err(|err: HexToArrayError| Error::UnexpectedResponse(err.to_string()))
    }

    /// Call a method on the Bitcoin Core compatible JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call fails.
    pub fn call(&self, method: &str, args: &[Value]) -> Result<Value, Error> {
        self.client
            .call(method, args)
            .map_err(|source| NodeError::JsonRpc(source).into())
    }

    /// Report that this wrapper does not support the peer lookup command.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::UnsupportedCommand`].
    pub fn has_peer(&self, _socket: SocketAddr) -> Result<bool, Error> {
        Err(Self::unsupported_node("has_peer"))
    }

    /// Report that this wrapper does not support the add-peer command.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::UnsupportedCommand`].
    pub fn add_peer(&self, _socket: SocketAddr) -> Result<(), Error> {
        Err(Self::unsupported_node("add_peer"))
    }

    /// Return the peer count from the JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call fails or does not return a `u32` count.
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let value = self.call("getconnectioncount", &[])?;
        value
            .as_u64()
            .and_then(|count| u32::try_from(count).ok())
            .ok_or_else(|| {
                Error::UnexpectedResponse("getconnectioncount returned a non-u32 count".to_string())
            })
    }

    /// Report that this wrapper does not have a separate index trigger.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::UnsupportedCommand`].
    pub fn trigger(&self) -> Result<(), Error> {
        Err(IndexerError::UnsupportedCommand {
            indexer: <Self as Indexer>::get_name(),
            command: "trigger",
        }
        .into())
    }

    /// Wait until the Electrum endpoint serves the chain tip of `node`.
    ///
    /// # Errors
    ///
    /// Returns an error if an endpoint fails or the time limit expires.
    pub fn wait_until_caught_up(
        &self,
        node: &impl Node,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let height = node.get_chain_tip()?;
        self.wait_until_tip(height, node.get_block_hash(height)?, timeout)
    }

    /// Make an error for a node command that `bs` does not support.
    fn unsupported_node(command: &'static str) -> Error {
        NodeError::UnsupportedCommand {
            node: <Self as Node>::get_name(),
            command,
        }
        .into()
    }

    /// Reject settings that this version of `bs` does not support.
    fn validate_conf(conf: &LibbitcoinDConf) -> Result<(), Error> {
        if conf.args.network != Network::Bitcoin {
            return Err(NodeError::InvalidConfiguration(
                "this pinned bs starts with mainnet consensus settings".to_string(),
            )
            .into());
        }
        if !conf.args.fixed_peers.is_empty()
            || conf.args.v2_transport
            || conf.args.cbf_index
            || conf.args.prune != PruneMode::Disabled
            || !conf.args.txindex
        {
            return Err(NodeError::InvalidConfiguration(
                "unsupported libbitcoin node arguments".to_string(),
            )
            .into());
        }
        if let Some(arg) = find_conflicting_argument(&conf.raw_args, &["config", "c"], &[]) {
            return Err(NodeError::ConflictingArgument(arg).into());
        }
        Ok(())
    }

    /// Write the private configuration file for `bs`.
    fn write_config(
        path: &Path,
        dir: &Path,
        p2p: SocketAddr,
        rpc: SocketAddr,
        electrum: SocketAddr,
        esplora: SocketAddr,
    ) -> Result<(), Error> {
        let mut content = String::new();
        content.push_str("[database]\n");
        content.push_str(&format!("path = {}\n", dir.join("database").display()));
        content.push_str("[peer]\n");
        content.push_str(&format!("path = {}\n", dir.join("network").display()));
        content.push_str("[node]\n");
        content.push_str("delay_inbound = false\n");
        content.push_str("[inbound]\n");
        content.push_str(&format!("bind = {p2p}\n"));
        content.push_str("connections = 8\n");
        content.push_str("[bitcoind]\n");
        content.push_str(&format!("bind = {rpc}\n"));
        content.push_str("connections = 16\n");
        content.push_str(&format!("credential = {RPC_USER}:{RPC_PASS}\n"));
        content.push_str("[electrum]\n");
        content.push_str(&format!("bind = {electrum}\n"));
        content.push_str("connections = 16\n");
        content.push_str("[esplora]\n");
        content.push_str(&format!("bind = {esplora}\n"));
        content.push_str("connections = 16\n");
        content.push_str("[log]\n");
        content.push_str(&format!("path = {}\n", dir.join("log").display()));

        fs::write(path, content).map_err(Error::Io)
    }

    /// Poll Electrum until it serves the block at `height` with the expected hash.
    ///
    /// # Errors
    ///
    /// Returns an error if Electrum fails or the time limit expires.
    pub fn wait_until_tip(
        &self,
        height: u32,
        hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let timeout = timeout.unwrap_or(INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            let tip = self
                .electrum_client
                .block_headers_subscribe()
                .map_err(|source| IndexerError::UnresponsiveIndexer {
                    indexer: <Self as Indexer>::get_name(),
                    source,
                })?;
            if tip.height >= height as usize {
                let header =
                    self.electrum_client
                        .block_header(height as usize)
                        .map_err(|source| IndexerError::UnresponsiveIndexer {
                            indexer: <Self as Indexer>::get_name(),
                            source,
                        })?;
                if header.block_hash() == hash {
                    return Ok(());
                }
            }
            sleep(POLL_INTERVAL);
        }
        Err(IndexerError::IndexingTimeout {
            indexer: <Self as Indexer>::get_name(),
            description: format!("block {height} ({hash})"),
            timeout,
        }
        .into())
    }

    /// Poll Electrum until its history includes the unconfirmed transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if Electrum fails or the time limit expires.
    pub fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let timeout = timeout.unwrap_or(INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            let history = self
                .electrum_client
                .script_get_history(spk)
                .map_err(|source| IndexerError::UnresponsiveIndexer {
                    indexer: <Self as Indexer>::get_name(),
                    source,
                })?;
            if history
                .iter()
                .any(|entry| entry.tx_hash == txid && entry.height == 0)
            {
                return Ok(());
            }
            sleep(POLL_INTERVAL);
        }
        Err(IndexerError::IndexingTimeout {
            indexer: <Self as Indexer>::get_name(),
            description: format!("mempool transaction {txid}"),
            timeout,
        }
        .into())
    }
}

impl Drop for LibbitcoinD {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = &self.working_directory;
    }
}
