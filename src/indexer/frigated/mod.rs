// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control a Frigate [`Indexer`] process.
//!
//! [`FrigateD`] uses a Bitcoin Core [`Node`] and another Electrum [`Indexer`].
//! Frigate needs an unpruned Bitcoin Core node with a transaction index.
//! It also needs an Electrum backend other than Frigate.
//!
//! # Start a [`FrigateD`] process
//!
//! ```rust,no_run
//! use halfin::indexer::Indexer;
//! use halfin::indexer::frigated::FrigateD;
//! use halfin::indexer::frigated::FrigateDConf;
//! use halfin::node::Node;
//!
//! fn start_frigate<N: Node, I: Indexer>(node: &N, backend: &I) {
//!     // Start with the default configuration.
//!     let default_indexer = FrigateD::new(node, backend).unwrap();
//!
//!     // Start with a custom configuration.
//!     let conf = FrigateDConf::default();
//!     let custom_indexer = FrigateD::new_with_conf(node, backend, &conf).unwrap();
//! }
//! ```
//!
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use core::time::Duration;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread::sleep;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::Txid;
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use tracing::debug;

use crate::DataDir;
use crate::Error;
use crate::INDEXING_TIMEOUT;
use crate::IPV4_LOCALHOST;
use crate::POLL_INTERVAL;
use crate::SPAWN_ATTEMPTS;
use crate::SPAWN_INTERVAL;
use crate::find_conflicting_argument;
use crate::get_available_port;
use crate::indexer::Indexer;
use crate::indexer::IndexerError;
use crate::indexer::ensure_backend_ready;
use crate::indexer::read_backend_cookie;
use crate::indexer::validate_backend;
use crate::init_data_dir;
use crate::node::Node;
use crate::node::NodeArgs;
use crate::node::PruneMode;
use crate::pipe_to_tracing;

#[cfg(test)]
mod test;

/// Bundled Frigate release metadata.
mod versions;

/// Path to the Frigate launcher in its bundled application image.
///
/// # Errors
/// Returns [`Error::BinaryNotFound`] when the downloaded bundle is unavailable.
pub fn get_frigate_path() -> Result<PathBuf, Error> {
    let path = PathBuf::from(option_env!("HALFIN_FRIGATE_PATH").unwrap_or(""));
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::BinaryNotFound((
            versions::FRIGATE_BIN_NAME.to_string(),
            path,
        )))
    }
}

/// Configuration for a Frigate instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrigateDConf {
    /// Additional Frigate CLI arguments. The wrapper rejects directory and network options
    /// that it sets itself.
    pub raw_args: Vec<String>,
    /// Root for a new temporary directory.
    pub tmpdir: Option<PathBuf>,
    /// Data directory that remains after the process stops. This is not removed on drop.
    pub staticdir: Option<PathBuf>,
    /// Maximum number of attempts to start Frigate with a fresh Electrum port.
    pub max_retries: u8,
}

impl Default for FrigateDConf {
    fn default() -> Self {
        Self {
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

/// A running Frigate Electrum server backed by another indexer.
#[derive(Debug)]
pub struct FrigateD<'a, I: Indexer> {
    /// Running Frigate process.
    process: Child,
    /// Electrum indexer used to serve standard methods.
    backend_indexer: &'a I,
    /// Client connected to Frigate's Electrum endpoint.
    client: RawClient<ElectrumPlaintextStream>,
    /// Data directory and cleanup state.
    working_directory: DataDir,
    /// Startup configuration.
    config: FrigateDConf,
    /// Frigate Electrum address.
    electrum_socket: SocketAddr,
}

impl<I: Indexer> FrigateD<'_, I> {
    /// Human-readable implementation name.
    pub fn get_name() -> &'static str {
        versions::FRIGATE_NAME
    }
    /// Launcher name.
    pub fn get_bin_name() -> &'static str {
        versions::FRIGATE_BIN_NAME
    }
}

impl<'a, I: Indexer> FrigateD<'a, I> {
    /// Start Frigate from the bundled application image.
    ///
    /// # Errors
    /// Returns an error if the bundle, configuration, or backend is invalid, or startup fails.
    pub fn new<N: Node>(node: &N, backend: &'a I) -> Result<Self, Error> {
        Self::new_with_conf(node, backend, &FrigateDConf::default())
    }

    /// Start Frigate from the bundled application image with configuration.
    ///
    /// # Errors
    /// Returns an error if the bundle, configuration, or backend is invalid, or startup fails.
    pub fn new_with_conf<N: Node>(
        node: &N,
        backend: &'a I,
        conf: &FrigateDConf,
    ) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_frigate_path()?, node, backend, conf)
    }

    /// Start a Frigate launcher at `bin`.
    ///
    /// # Errors
    /// Returns an error if the launcher, configuration, or backend is invalid, or startup fails.
    pub fn from_bin<P: AsRef<Path>, N: Node>(
        bin: P,
        node: &N,
        backend: &'a I,
    ) -> Result<Self, Error> {
        Self::from_bin_with_conf(bin, node, backend, &FrigateDConf::default())
    }

    /// Start a Frigate launcher at `bin` with configuration.
    ///
    /// # Errors
    /// Returns an error if the launcher, configuration, or backend is invalid, or startup fails.
    pub fn from_bin_with_conf<P: AsRef<Path>, N: Node>(
        bin: P,
        node: &N,
        backend_indexer: &'a I,
        conf: &FrigateDConf,
    ) -> Result<Self, Error> {
        validate_backend::<N>()?;
        if I::get_name() == Self::get_name() {
            return Err(IndexerError::InvalidConfiguration(
                "Frigate cannot use another Frigate as its Electrum backend".to_string(),
            )
            .into());
        }
        if let Some(arg) =
            find_conflicting_argument(&conf.raw_args, &["d", "dir", "n", "network"], &[])
        {
            return Err(IndexerError::ConflictingArgument(arg).into());
        }
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
        let args = node.get_config().as_ref();
        validate_node_args(args)?;
        let (_, cookie) = read_backend_cookie(node)?;
        ensure_backend_ready(node, args.network, Self::get_name())?;
        let network = args.network.to_string();
        let network_home = match args.network {
            Network::Bitcoin => "",
            Network::Testnet => "testnet3",
            Network::Testnet4 | Network::Signet | Network::Regtest => network.as_str(),
            #[allow(unreachable_patterns)]
            _ => {
                return Err(IndexerError::InvalidConfiguration(format!(
                    "unsupported network: {}",
                    args.network
                ))
                .into());
            }
        };
        let rpc_socket = node.get_rpc_socket();

        for _ in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-frigate-",
            )?;
            let electrum_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port()));
            let config_directory = working_directory.path().join(network_home);
            fs::create_dir_all(&config_directory).map_err(Error::Io)?;
            let contents = format!(
                "[core]\nconnect = true\nserver = {}\nauthType = \"USERPASS\"\nauth = {}\n\n[server]\ntcp = {}\nbackendElectrumServer = {}\n",
                toml_string(&format!("http://{rpc_socket}")),
                toml_string(&cookie),
                toml_string(&format!("tcp://{electrum_socket}")),
                toml_string(&format!("tcp://{}", backend_indexer.get_electrum_socket()))
            );
            fs::write(config_directory.join("config.toml"), contents).map_err(Error::Io)?;
            let mut command = Command::new(bin);
            command.arg("-d").arg(working_directory.path());
            if args.network != Network::Bitcoin {
                command.arg("-n").arg(&network);
            }
            let mut process = command
                .args(&conf.raw_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;
            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "frigate");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "frigate");
            }
            sleep(SPAWN_INTERVAL);
            if !matches!(process.try_wait(), Ok(None)) {
                let _ = process.kill();
                let _ = process.wait();
                continue;
            }
            if let Ok(client) = wait_for_client(electrum_socket, &mut process, INDEXING_TIMEOUT) {
                return Ok(Self {
                    process,
                    backend_indexer,
                    client,
                    working_directory,
                    config: conf.clone(),
                    electrum_socket,
                });
            }
            let _ = process.kill();
            let _ = process.wait();
        }
        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Ask the backing indexer to scan for new data.
    ///
    /// # Errors
    /// Returns the backing indexer's trigger error.
    pub fn trigger(&self) -> Result<(), Error> {
        self.backend_indexer.trigger()
    }

    /// Stop Frigate and return its exit status.
    ///
    /// # Errors
    /// Returns an I/O error if the wrapper cannot wait for the child process.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        let _ = self.process.kill();
        self.process.wait().map_err(Error::Io)
    }

    /// Return the Frigate process ID.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }
    /// Return Frigate's data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }
    /// Return the configuration used to start Frigate.
    pub fn get_config(&self) -> &FrigateDConf {
        &self.config
    }
    /// Return the Electrum client connected to Frigate.
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        &self.client
    }
    /// Return Frigate's Electrum socket.
    pub fn get_electrum_socket(&self) -> SocketAddr {
        self.electrum_socket
    }
    /// Return Frigate's Electrum URL.
    pub fn get_electrum_url(&self) -> String {
        self.electrum_socket.to_string()
    }

    /// Wait until Frigate reaches the node's current chain tip.
    ///
    /// # Errors
    /// Returns an error if querying the node or Frigate fails, or the timeout expires.
    pub fn wait_until_caught_up(
        &self,
        node: &impl Node,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let height = node.get_chain_tip()?;
        self.wait_until_tip(height, node.get_block_hash(height)?, timeout)
    }

    /// Wait until Frigate reports a specific block at a specific height.
    ///
    /// # Errors
    /// Returns an error if a Frigate request fails or the time limit expires.
    pub fn wait_until_tip(
        &self,
        height: u32,
        hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let timeout = timeout.unwrap_or(INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger()?;
            match self.client.block_header(height as usize) {
                Ok(header) if header.block_hash() == hash => return Ok(()),
                Ok(_) => {}
                Err(err) if is_incomplete_read(&err) => {}
                Err(err) => return Err(unresponsive_indexer(err).into()),
            }
            sleep(2 * POLL_INTERVAL);
        }
        Err(IndexerError::IndexingTimeout {
            indexer: Self::get_name(),
            description: format!("block {height} ({hash})"),
            timeout,
        }
        .into())
    }

    /// Wait until Frigate reports an unconfirmed transaction in a script's history.
    ///
    /// # Errors
    /// Returns an error if a Frigate request fails or the time limit expires.
    pub fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let timeout = timeout.unwrap_or(INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger()?;
            match self.client.script_get_history(spk) {
                Ok(history)
                    if history
                        .iter()
                        .any(|item| item.tx_hash == txid && item.height == 0) =>
                {
                    return Ok(());
                }
                Ok(_) => {}
                Err(err) if is_incomplete_read(&err) => {}
                Err(err) => return Err(unresponsive_indexer(err).into()),
            }
            sleep(2 * POLL_INTERVAL);
        }
        Err(IndexerError::IndexingTimeout {
            indexer: Self::get_name(),
            description: format!("mempool transaction {txid}"),
            timeout,
        }
        .into())
    }
}

impl<I: Indexer> Drop for FrigateD<'_, I> {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[rustfmt::skip]
impl<I: Indexer> Indexer for FrigateD<'_, I> {
    type Config = FrigateDConf;
    fn get_name() -> &'static str { Self::get_name() }
    fn get_bin_name() -> &'static str { Self::get_bin_name() }
    fn trigger(&self) -> Result<(), Error> { self.trigger() }
    fn stop(&mut self) -> Result<ExitStatus, Error> { self.stop() }
    fn get_pid(&self) -> u32 { self.get_pid() }
    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }
    fn get_config(&self) -> &FrigateDConf { self.get_config() }
    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> { self.get_electrum_client() }
    fn get_electrum_socket(&self) -> SocketAddr { self.get_electrum_socket() }
    fn get_electrum_url(&self) -> String { self.get_electrum_url() }
    fn wait_until_caught_up(&self, node: &impl Node, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_caught_up(node, timeout) }
    fn wait_until_tip(&self, height: u32, hash: BlockHash, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_tip(height, hash, timeout) }
    fn wait_until_mempool_tx(&self, spk: &Script, txid: Txid, timeout: Option<Duration>) -> Result<(), Error> { self.wait_until_mempool_tx(spk, txid, timeout) }
}

/// Reject node configurations that cannot provide Frigate's indexed history.
fn validate_node_args(args: &NodeArgs) -> Result<(), Error> {
    if args.prune != PruneMode::Disabled || !args.txindex {
        return Err(IndexerError::InvalidConfiguration(
            "Frigate requires an unpruned node with txindex enabled".to_string(),
        )
        .into());
    }
    Ok(())
}

/// Quote a configuration value using TOML-compatible string escapes.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

/// Add Frigate context to an Electrum client failure.
fn unresponsive_indexer(source: ElectrumError) -> IndexerError {
    IndexerError::UnresponsiveIndexer {
        indexer: versions::FRIGATE_NAME,
        source,
    }
}

/// Recognize short reads during indexing and network retries.
fn is_incomplete_read(err: &ElectrumError) -> bool {
    matches!(err, ElectrumError::IOError(io_err) if matches!(io_err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe))
}

/// Wait until Frigate's Electrum endpoint accepts requests.
fn wait_for_client(
    socket: SocketAddr,
    process: &mut Child,
    timeout: Duration,
) -> Result<RawClient<ElectrumPlaintextStream>, Error> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !matches!(process.try_wait(), Ok(None)) {
            return Err(Error::ClientSetupTimeout);
        }
        if let Ok(client) = RawClient::new(socket, Some(Duration::from_millis(500)), None) {
            if client.ping().is_ok() {
                return Ok(client);
            }
        }
        sleep(Duration::from_millis(200));
    }
    debug!("Frigate did not become responsive at {socket}");
    Err(Error::ClientSetupTimeout)
}
