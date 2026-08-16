// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control an `electrs` [`Indexer`] process.
//!
//! [`ElectrsD`] starts `electrs` and connects it to a local [`Node`].
//! It gives an Electrum client and wait operations for integration tests.
//!
//! ## Start an [`Indexer`]
//!
//! ```rust,no_run
//! use halfin::indexer::electrsd::ElectrsD;
//! use halfin::node::Node;
//!
//! fn start_electrs(node: &impl Node) {
//!     node.generate(10).unwrap();
//!     let electrs = ElectrsD::new(node).unwrap();
//!     electrs.wait_until_caught_up(node, None).unwrap();
//! }
//! ```
//!
//! ## Select a data directory
//!
//! By default, each [`ElectrsD`] instance uses a temporary directory.
//! [`Drop`] removes this directory.
//! Set [`ElectrsDConf::staticdir`] to keep the data after the process stops.
//!
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::Txid;
use corepc_client::bitcoin::block::Header;
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
use electrum_client::HeaderNotification;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use tracing::debug;

use crate::DataDir;
use crate::Error;
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

/// Bundled `electrs` version metadata.
mod versions;

/// The default timeout for [`ElectrsD`] indexing helpers.
pub const ELECTRS_INDEXING_TIMEOUT: Duration = Duration::from_secs(30);

/// Wrap an Electrum client failure with [`Indexer`] context.
fn unresponsive_indexer(source: ElectrumError) -> IndexerError {
    IndexerError::UnresponsiveIndexer {
        indexer: ElectrsD::get_name(),
        source,
    }
}

/// Return the path to the downloaded `electrs` binary.
///
/// At compile time, `build.rs` reads and extracts the local archive.
/// It stores the binary path in `HALFIN_ELECTRS_PATH`.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the compiled-in binary path does not exist.
pub fn get_electrs_path() -> Result<PathBuf, Error> {
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_ELECTRS_PATH").unwrap_or(""));

    // Add the `.exe` suffix on Windows.
    #[cfg(target_os = "windows")]
    if bin_path.extension().is_none() {
        bin_path.set_extension("exe");
    }

    let bin_name = ElectrsD::get_bin_name().to_string();
    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

/// Configuration for an [`ElectrsD`] instance.
///
/// Specify each field or use [`ElectrsDConf::default`] for standard regtest values.
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
pub struct ElectrsDConf {
    /// Extra CLI arguments sent unchanged to the `electrs` process.
    ///
    /// Do not use a raw argument for an option that `halfin` controls.
    /// A duplicate option returns [`IndexerError::ConflictingArgument`].
    pub raw_args: Vec<String>,

    /// Root for the new temporary directory of each instance.
    /// If this field is empty, the function uses `TEMPDIR_ROOT`.
    /// If `TEMPDIR_ROOT` is empty, the function uses the system temporary directory.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory.
    /// The function creates the directory if necessary.
    /// [`Drop`] stops the process but keeps the files.
    pub staticdir: Option<PathBuf>,

    /// Maximum number of attempts to start `electrs`.
    ///
    /// Each attempt uses new random ports. Thus, a new attempt can correct a temporary port
    /// conflict. The default value is [`SPAWN_ATTEMPTS`].
    pub max_retries: u8,
}

impl Default for ElectrsDConf {
    fn default() -> Self {
        Self {
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

/// A running `electrs` [`Indexer`].
///
/// [`ElectrsD::from_bin`] and related functions start the [`Indexer`].
/// The [`Indexer`] connects to the specified [`Node`].
/// [`Drop`] stops the [`Indexer`].
///
/// # Networking
///
/// At startup, the operating system selects temporary Electrum RPC and monitoring ports.
/// Use [`get_electrum_socket`](ElectrsD::get_electrum_socket) to get the Electrum RPC port.
/// Use [`get_monitoring_socket`](ElectrsD::get_monitoring_socket) to get the monitoring port.
#[derive(Debug)]
pub struct ElectrsD {
    /// Handle for the `electrs` child process.
    process: Child,

    /// Plaintext Electrum client connected to `electrs`.
    pub client: RawClient<ElectrumPlaintextStream>,

    /// Data directory of the [`Indexer`] and its cleanup state.
    working_directory: DataDir,

    /// Complete configuration used to start the [`Indexer`].
    config: ElectrsDConf,

    /// Address of the Electrum RPC server.
    electrum_socket: SocketAddr,

    /// Address of the monitoring server.
    monitoring_socket: SocketAddr,
}

#[rustfmt::skip]
impl Indexer for ElectrsD {
    type Config = ElectrsDConf;

    fn get_name() -> &'static str { Self::get_name() }

    fn get_bin_name() -> &'static str { Self::get_bin_name() }

    fn trigger(&self) -> Result<(), Error> { self.trigger() }

    fn stop(&mut self) -> Result<std::process::ExitStatus, Error> { self.stop() }

    fn get_pid(&self) -> u32 { self.get_pid() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_config(&self) -> &ElectrsDConf { self.get_config() }

    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> { self.get_electrum_client() }

    fn get_electrum_socket(&self) -> SocketAddr { self.get_electrum_socket() }

    fn get_electrum_url(&self) -> String { self.get_electrum_url() }

    fn wait_until_caught_up(&self, node: &impl Node, timeout: Option<Duration>) -> Result<(), Error> {
        self.wait_until_caught_up(node, timeout)
    }

    fn wait_until_tip(&self, exp_height: u32, exp_hash: BlockHash, timeout: Option<Duration>) -> Result<(), Error> {
        self.wait_until_tip(exp_height, exp_hash, timeout)
    }

    fn wait_until_mempool_tx(&self, spk: &Script, txid: Txid, timeout: Option<Duration>) -> Result<(), Error> {
        self.wait_until_mempool_tx(spk, txid, timeout)
    }
}

#[rustfmt::skip]
impl ElectrsD {
    /// Human-readable name of [`ElectrsD`].
    pub fn get_name() -> &'static str { versions::ELECTRS_NAME }

    /// Binary name of [`ElectrsD`].
    pub fn get_bin_name() -> &'static str { versions::ELECTRS_BIN_NAME }
}

impl ElectrsD {
    /// Start an [`ElectrsD`] [`Indexer`] with the binary from [`get_electrs_path`].
    /// Use the default [`ElectrsDConf`].
    ///
    /// The [`Indexer`] connects to the specified [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the [`Indexer`].
    /// Returns an error if the [`Node`] is not ready.
    pub fn new<N: Node>(node: &N) -> Result<Self, Error> {
        Self::from_bin(get_electrs_path()?, node)
    }

    /// Start an [`ElectrsD`] [`Indexer`] with the binary from [`get_electrs_path`].
    /// Use the specified [`ElectrsDConf`].
    ///
    /// The [`Indexer`] connects to the specified [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the [`Indexer`].
    /// Returns an error if the configuration is not valid or the [`Node`] is not ready.
    pub fn new_with_conf<N: Node>(node: &N, conf: &ElectrsDConf) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_electrs_path()?, node, conf)
    }

    /// Start the binary at [`Path`] with the default [`ElectrsDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if `electrs_bin` is not valid or the [`Node`] is not ready.
    /// Returns an error if the function cannot start the [`Indexer`].
    pub fn from_bin<P: AsRef<Path>, N: Node>(electrs_bin: P, node: &N) -> Result<Self, Error> {
        Self::from_bin_with_conf(electrs_bin, node, &ElectrsDConf::default())
    }

    /// Start the binary at [`Path`] with the specified [`ElectrsDConf`].
    ///
    /// The method uses at most [`ElectrsDConf::max_retries`] attempts.
    ///
    /// 1. Select new temporary Electrum and monitoring ports.
    /// 2. Start `electrs` with the RPC and P2P sockets of the specified [`Node`].
    /// 3. Wait a maximum of 10 seconds for the Electrum RPC server to respond.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is not valid or the [`Node`] is not ready.
    /// Returns an error if directory creation or all start attempts fail.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>, N: Node>(
        electrs_bin: P,
        node: &N,
        conf: &ElectrsDConf,
    ) -> Result<Self, Error> {
        validate_backend::<N>()?;
        let node_args = node.get_config().as_ref();
        let configured_args = Self::configured_args(conf, node_args.network)?;

        // Validate the `electrs_bin` path.
        let electrs_bin = electrs_bin.as_ref();
        // The path must be absolute.
        if !electrs_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: electrs_bin.display().to_string(),
            });
        }
        // The path must be a file.
        if !electrs_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: electrs_bin.display().to_string(),
            });
        }

        Self::validate_node_args(node_args)?;
        let (cookie_file, _) = read_backend_cookie(node)?;
        ensure_backend_ready(node, node_args.network, Self::get_name())?;
        let node_rpc_socket = node.get_rpc_socket();
        let node_p2p_socket = node.get_p2p_socket();

        for _attempt in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-electrs-",
            )?;

            let electrum_port = get_available_port();
            let electrum_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, electrum_port));

            let monitoring_port = get_available_port();
            let monitoring_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, monitoring_port));
            let mut args = configured_args.clone();
            args.extend(conf.raw_args.iter().cloned());
            args.extend([
                "--db-dir".to_string(),
                working_directory.path().display().to_string(),
                "--daemon-rpc-addr".to_string(),
                node_rpc_socket.to_string(),
                "--daemon-p2p-addr".to_string(),
                node_p2p_socket.to_string(),
                "--electrum-rpc-addr".to_string(),
                electrum_socket.to_string(),
                "--monitoring-addr".to_string(),
                monitoring_socket.to_string(),
                "--cookie-file".to_string(),
                cookie_file.display().to_string(),
            ]);

            debug!(
                "Spawning {} [ELECTRUM_SOCKET={}, MONITORING_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                electrum_socket,
                monitoring_socket,
                working_directory.path().display()
            );

            let mut process = Command::new(electrs_bin)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            // Pipe the indexer's stdout/stderr into `tracing` so its logs are
            // visible alongside halfin's own. The reader threads exit on EOF
            // when the process dies.
            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "electrs");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "electrs");
            }

            // Add a small timeout to let `electrs` fail
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

            if let Ok(client) =
                Self::wait_for_client(electrum_socket, &mut process, Duration::from_secs(10))
            {
                sleep(Duration::from_millis(200));

                debug!(
                    "Started {} [PID={}, ELECTRUM_SOCKET={}, MONITORING_SOCKET={}, DATADIR={}]",
                    Self::get_name(),
                    process.id(),
                    electrum_socket,
                    monitoring_socket,
                    working_directory.path().display()
                );

                return Ok(Self {
                    process,
                    client,
                    working_directory,
                    config: conf.clone(),
                    electrum_socket,
                    monitoring_socket,
                });
            }
            let _ = process.kill();
            let _ = process.wait();
        }

        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Send `SIGUSR1` to trigger a rescan on Unix-derived platforms.
    ///
    /// This method does nothing on Windows.
    ///
    /// # Errors
    ///
    /// Returns an error if the signal command cannot run or returns an error status.
    #[cfg(not(target_os = "windows"))]
    pub fn trigger(&self) -> Result<(), Error> {
        debug!(
            "{}: triggering rescan pid={}",
            Self::get_name(),
            self.process.id()
        );

        let status = Command::new("kill")
            .arg("-USR1")
            .arg(self.process.id().to_string())
            .status()
            .map_err(Error::Io)?;
        if status.success() {
            debug!("{}: triggered rescan", Self::get_name());

            Ok(())
        } else {
            Err(Error::UnexpectedResponse(format!(
                "failed to trigger electrs rescan with exit status={status}"
            )))
        }
    }

    /// Do not start a rescan on Windows.
    ///
    /// # Errors
    ///
    /// This implementation does not return an error.
    #[cfg(target_os = "windows")]
    pub fn trigger(&self) -> Result<(), Error> {
        debug!("{}: skipped rescan trigger on Windows", Self::get_name());

        Ok(())
    }

    /// Terminate the `electrs` process and wait for it to exit.
    ///
    /// [`Drop`] stops the process without a call to this method.
    /// Call this method to get the exit status or confirm that the process has stopped.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot wait for the child process.
    pub fn stop(&mut self) -> Result<std::process::ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());
        let _ = self.process.kill();
        self.process.wait().map_err(Error::Io)
    }

    /// Return the operating system process ID of `electrs`.
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", Self::get_name(), pid);

        pid
    }

    /// Return the data directory of [`ElectrsD`].
    pub fn get_working_directory(&self) -> PathBuf {
        let working_directory = self.working_directory.path();

        debug!(
            "{}: got working directory at path={}",
            Self::get_name(),
            working_directory.display()
        );

        working_directory
    }

    /// Return the complete configuration used to start this [`Indexer`].
    pub fn get_config(&self) -> &ElectrsDConf {
        &self.config
    }

    /// Return a reference to the Electrum [`RawClient`] of [`ElectrsD`].
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        debug!(
            "{}: got electrum client for socket={}",
            Self::get_name(),
            self.electrum_socket
        );

        &self.client
    }

    /// Return the Electrum RPC [`SocketAddr`] of the [`Indexer`].
    pub fn get_electrum_socket(&self) -> SocketAddr {
        debug!(
            "{}: got electrum socket at socket={}",
            Self::get_name(),
            self.electrum_socket
        );

        self.electrum_socket
    }

    /// Return the Electrum RPC URL for the [`Indexer`].
    pub fn get_electrum_url(&self) -> String {
        let electrum_url = self.electrum_socket.to_string();

        debug!(
            "{}: got electrum url at url={}",
            Self::get_name(),
            electrum_url
        );

        electrum_url
    }

    /// Return the monitoring [`SocketAddr`] of the [`Indexer`].
    pub fn get_monitoring_socket(&self) -> SocketAddr {
        debug!(
            "{}: got monitoring socket at socket={}",
            Self::get_name(),
            self.monitoring_socket
        );

        self.monitoring_socket
    }

    /// Poll until the Electrum header tip matches the tip of a [`Node`].
    ///
    /// The function verifies the tip height and block hash.
    /// Specify `None` to use [`ELECTRS_INDEXING_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot query the [`Node`].
    /// Returns an error if the [`Indexer`] does not reach the [`Node`] tip before the timeout.
    pub fn wait_until_caught_up(
        &self,
        node: &impl Node,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let height = node.get_chain_tip()?;
        let hash = node.get_block_hash(height)?;

        debug!(
            "{}: waiting until caught up height={} hash={}",
            Self::get_name(),
            height,
            hash
        );

        self.wait_until_block(height, Some(hash), timeout)
    }

    /// Poll until the Electrum header tip of [`ElectrsD`] reaches `exp_height`.
    ///
    /// The function compares the block hash at `exp_height` with `exp_hash`.
    /// Specify `None` to use [`ELECTRS_INDEXING_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot query the [`Indexer`].
    /// Returns an error if the [`Indexer`] does not reach the expected tip before the timeout.
    pub fn wait_until_tip(
        &self,
        exp_height: u32,
        exp_hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        debug!(
            "{}: waiting until tip height={} hash={}",
            Self::get_name(),
            exp_height,
            exp_hash
        );

        self.wait_until_block(exp_height, Some(exp_hash), timeout)
    }

    /// Poll until the history of `spk` contains `txid` as an unconfirmed transaction.
    ///
    /// If `timeout` is `None`, the function uses [`ELECTRS_INDEXING_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if Electrum history calls fail or the transaction does
    /// not appear before the timeout.
    pub fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        debug!(
            "{}: waiting until mempool transaction txid={}",
            Self::get_name(),
            txid
        );

        let timeout = timeout.unwrap_or(ELECTRS_INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger()?;

            if Self::script_history_has_mempool_tx(&self.client, spk, txid)? {
                debug!(
                    "{}: found mempool transaction with txid={}",
                    Self::get_name(),
                    txid
                );

                return Ok(());
            }

            sleep(2 * POLL_INTERVAL);
        }

        Err(IndexerError::IndexingTimeout {
            indexer: Self::get_name(),
            description: format!("mempool transaction with txid={txid}"),
            timeout,
        }
        .into())
    }

    /// Validate raw arguments and create [`Indexer`] arguments.
    fn configured_args(conf: &ElectrsDConf, network: Network) -> Result<Vec<String>, Error> {
        const OPTIONS: &[&str] = &[
            "cookie-file",
            "daemon-p2p-addr",
            "daemon-rpc-addr",
            "db-dir",
            "electrum-rpc-addr",
            "monitoring-addr",
            "network",
        ];

        if let Some(arg) = find_conflicting_argument(&conf.raw_args, OPTIONS, &[]) {
            return Err(IndexerError::ConflictingArgument(arg).into());
        }

        let network = match network {
            Network::Bitcoin => "bitcoin",
            Network::Testnet => "testnet",
            Network::Testnet4 => "testnet4",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
        };

        Ok(vec!["--network".to_string(), network.to_string()])
    }

    /// Return whether the history of `spk` contains `txid` as an unconfirmed transaction.
    fn script_history_has_mempool_tx(
        client: &RawClient<ElectrumPlaintextStream>,
        spk: &Script,
        txid: Txid,
    ) -> Result<bool, Error> {
        match client.script_get_history(spk) {
            Ok(history) => {
                let has_tx = history
                    .iter()
                    .any(|entry| entry.tx_hash == txid && entry.height == 0);

                debug!(
                    "{}: checked script mempool transaction with txid={} found={}",
                    Self::get_name(),
                    txid,
                    has_tx
                );

                Ok(has_tx)
            }
            Err(err) if is_incomplete_read(&err) => Ok(false),
            Err(err) => Err(unresponsive_indexer(err).into()),
        }
    }

    /// Wait for an Electrum block header notification for `exp_height` and `exp_hash`.
    ///
    /// Electrs sends a header notification after it updates confirmed script histories for that
    /// tip. Thus, this function uses notifications and does not poll block headers directly.
    fn wait_until_block(
        &self,
        exp_height: u32,
        exp_hash: Option<BlockHash>,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let client = self.get_electrum_client();
        let mut next_notification = Some(
            client
                .block_headers_subscribe()
                .map_err(unresponsive_indexer)?,
        );

        let description = match exp_hash {
            Some(hash) => format!("block {exp_height} ({hash})"),
            None => format!("block {exp_height}"),
        };

        let timeout = timeout.unwrap_or(ELECTRS_INDEXING_TIMEOUT);
        debug!(
            "{}: waiting until indexed {} timeout={:?}",
            Self::get_name(),
            description,
            timeout
        );

        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger()?;
            match client.ping() {
                Ok(()) => {}
                Err(err) if is_incomplete_read(&err) => {
                    sleep(2 * POLL_INTERVAL);
                    continue;
                }
                Err(err) => return Err(unresponsive_indexer(err).into()),
            }

            let notification = match next_notification.take() {
                Some(notification) => Some(notification),
                None => match client.block_headers_pop() {
                    Ok(notification) => notification,
                    Err(err) if is_incomplete_read(&err) => None,
                    Err(err) => return Err(unresponsive_indexer(err).into()),
                },
            };
            let Some(notification) = notification else {
                sleep(2 * POLL_INTERVAL);
                continue;
            };

            if electrs_header_matches(client, &notification, exp_height, exp_hash)? {
                debug!("{}: finished indexing {}", Self::get_name(), description);

                return Ok(());
            }

            sleep(2 * POLL_INTERVAL);
        }

        Err(IndexerError::IndexingTimeout {
            indexer: Self::get_name(),
            description,
            timeout,
        }
        .into())
    }

    /// Reject [`Node`] configurations that electrs cannot index.
    fn validate_node_args(args: &NodeArgs) -> Result<(), Error> {
        if args.prune != PruneMode::Disabled {
            return Err(IndexerError::InvalidConfiguration(
                "electrs requires an unpruned backing node".to_string(),
            )
            .into());
        }
        Ok(())
    }

    /// Poll `server.ping` until it succeeds.
    /// Then, create and return the Electrum client.
    ///
    /// Returns `Err` if the [`Indexer`] is not responsive within `timeout`.
    fn wait_for_client(
        electrum_socket: SocketAddr,
        process: &mut Child,
        timeout: Duration,
    ) -> Result<RawClient<ElectrumPlaintextStream>, Error> {
        let start = Instant::now();
        let mut last_error = None;
        while start.elapsed() < timeout {
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    return Err(Error::ClientSetupTimeout);
                }
                Ok(None) => {}
            }

            match RawClient::new(electrum_socket, Some(Duration::from_millis(500)), None) {
                Ok(client) => match client.ping() {
                    Ok(()) => return Ok(client),
                    Err(err) => last_error = Some(err),
                },
                Err(err) => last_error = Some(err),
            }
            sleep(Duration::from_millis(200));
        }

        Err(last_error.map_or(Error::ClientSetupTimeout, |source| {
            unresponsive_indexer(source).into()
        }))
    }
}

impl Drop for ElectrsD {
    /// Terminate the `electrs` process and wait for it to exit.
    ///
    /// Ignore errors from `kill` and `wait` to prevent a panic in [`Drop`].
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            Self::get_name(),
            self.process.id()
        );
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Check whether an Electrum header notification shows that [`ElectrsD`] indexed `exp_height`.
///
/// If the notification is above `exp_height`, get the header at `exp_height`.
/// Then, compare its hash with `exp_hash`.
fn electrs_header_matches(
    client: &RawClient<ElectrumPlaintextStream>,
    notification: &HeaderNotification,
    exp_height: u32,
    exp_hash: Option<BlockHash>,
) -> Result<bool, Error> {
    electrs_header_matches_with(notification, exp_height, exp_hash, |height| {
        client.block_header(height)
    })
}

/// Check an Electrum header notification with an injected historical-header lookup.
fn electrs_header_matches_with<F>(
    notification: &HeaderNotification,
    exp_height: u32,
    exp_hash: Option<BlockHash>,
    get_header: F,
) -> Result<bool, Error>
where
    F: FnOnce(usize) -> Result<Header, ElectrumError>,
{
    let notification_height = u32::try_from(notification.height)
        .map_err(|err| Error::UnexpectedResponse(err.to_string()))?;

    if notification_height < exp_height {
        return Ok(false);
    }

    let header = if notification_height == exp_height {
        notification.header
    } else {
        match get_header(
            usize::try_from(exp_height)
                .map_err(|err| Error::UnexpectedResponse(err.to_string()))?,
        ) {
            Ok(header) => header,
            Err(err) if is_incomplete_read(&err) => return Ok(false),
            Err(err) => return Err(unresponsive_indexer(err).into()),
        }
    };

    Ok(exp_hash.is_none_or(|exp_hash| header.block_hash() == exp_hash))
}

/// Return whether an Electrum client error means that the socket has no complete response.
fn is_incomplete_read(err: &ElectrumError) -> bool {
    matches!(
        err,
        ElectrumError::IOError(io_err)
            if matches!(
                io_err.kind(),
                ErrorKind::WouldBlock
                    | ErrorKind::TimedOut
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::BrokenPipe
            )
    ) || matches!(
        err,
        ElectrumError::SharedIOError(io_err)
            if matches!(
                io_err.kind(),
                ErrorKind::WouldBlock
                    | ErrorKind::TimedOut
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::BrokenPipe
            )
    )
}

#[cfg(all(test, halfin_indexer))]
mod test;
