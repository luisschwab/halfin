// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control an `ElectrumX` [`Indexer`] process.
//!
//! [`ElectrumxD`] starts `ElectrumX` and connects it to a local [`Node`].
//! It gives Electrum and administration clients for integration tests.
//!
//! By default, each [`ElectrumxD`] instance uses a temporary directory.
//! [`Drop`] removes this directory.
//! Set [`ElectrumxDConf::staticdir`] to keep the data after the process stops.
//!
//! The bundled launcher requires Python 3.10.
//! On Windows ARM64, it requires Python 3.11.
//!
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::env;
use std::io::BufRead;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Write;
use std::net::TcpStream;
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
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
use electrum_client::ScriptStatus;
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
use crate::pipe_to_tracing;

/// Bundled `ElectrumX` version metadata.
mod versions;

/// The default timeout for [`ElectrumxD`] indexing helpers.
pub const ELECTRUMX_INDEXING_TIMEOUT: Duration = Duration::from_secs(30);

/// Wrap an Electrum client failure with [`Indexer`] context.
fn unresponsive_indexer(source: ElectrumError) -> IndexerError {
    IndexerError::UnresponsiveIndexer {
        indexer: ElectrumxD::get_name(),
        source,
    }
}

/// Return the path to the locally extracted `ElectrumX` launcher.
///
/// At compile time, `build.rs` extracts the local archive from `contrib/compile_electrumx`.
/// It stores the launcher path in `HALFIN_ELECTRUMX_PATH`.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the compiled-in binary path does not exist.
pub fn get_electrumx_path() -> Result<PathBuf, Error> {
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_ELECTRUMX_PATH").unwrap_or(""));

    #[cfg(target_os = "windows")]
    if bin_path.extension().is_none() {
        bin_path.set_extension("exe");
    }

    let bin_name = ElectrumxD::get_bin_name().to_string();
    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

/// Arguments specific to [`ElectrumxD`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ElectrumxDArgs {
    /// `ElectrumX` coin implementation name.
    pub coin: String,
}

/// Configuration for an [`ElectrumxD`] instance.
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
pub struct ElectrumxDConf {
    /// Arguments specific to `ElectrumX`.
    pub electrumx_args: ElectrumxDArgs,

    /// Extra CLI arguments sent unchanged to the `ElectrumX` launcher.
    ///
    /// Do not use a raw argument for an option in [`electrumx_args`](Self::electrumx_args).
    /// Do not duplicate an option that `halfin` controls.
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

    /// Maximum number of attempts to start `ElectrumX`.
    ///
    /// Each attempt uses new random ports. Thus, a new attempt can correct a temporary port
    /// conflict. The default value is [`SPAWN_ATTEMPTS`].
    pub max_retries: u8,
}

impl Default for ElectrumxDConf {
    fn default() -> Self {
        Self {
            electrumx_args: ElectrumxDArgs {
                coin: "Bitcoin".to_string(),
            },
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

/// A running `ElectrumX` [`Indexer`].
#[derive(Debug)]
pub struct ElectrumxD {
    /// Handle for the `ElectrumX` child process.
    process: Child,

    /// Plaintext Electrum client connected to `ElectrumX`.
    pub client: RawClient<ElectrumPlaintextStream>,

    /// Data directory of the [`Indexer`] and its cleanup state.
    working_directory: DataDir,

    /// Complete configuration used to start the [`Indexer`].
    config: ElectrumxDConf,

    /// Address of the Electrum RPC server.
    electrum_socket: SocketAddr,

    /// Address of the admin RPC server.
    rpc_socket: SocketAddr,
}

#[rustfmt::skip]
impl Indexer for ElectrumxD {
    type Config = ElectrumxDConf;

    fn get_name() -> &'static str { Self::get_name() }

    fn get_bin_name() -> &'static str { Self::get_bin_name() }

    fn trigger(&self) -> Result<(), Error> { self.trigger() }

    fn stop(&mut self) -> Result<std::process::ExitStatus, Error> { self.stop() }

    fn get_pid(&self) -> u32 { self.get_pid() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_config(&self) -> &ElectrumxDConf { self.get_config() }

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
impl ElectrumxD {
    /// Human-readable name of [`ElectrumxD`].
    pub fn get_name() -> &'static str { versions::ELECTRUMX_NAME }

    /// Binary name of [`ElectrumxD`].
    pub fn get_bin_name() -> &'static str { versions::ELECTRUMX_BIN_NAME }
}

impl ElectrumxD {
    /// Start an [`ElectrumxD`] [`Indexer`] with the binary from [`get_electrumx_path`].
    /// Use the default [`ElectrumxDConf`].
    ///
    /// The [`Indexer`] connects to the specified [`Node`].
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the required Python version is not available.
    /// Returns an error if the function cannot find the binary or start the [`Indexer`].
    /// Returns an error if the [`Node`] is not ready.
    pub fn new<N: Node>(node: &N) -> Result<Self, Error> {
        Self::new_with_conf(node, &ElectrumxDConf::default())
    }

    /// Start an [`ElectrumxD`] [`Indexer`] with the binary from [`get_electrumx_path`].
    /// Use the specified [`ElectrumxDConf`].
    ///
    /// The [`Indexer`] connects to the specified [`Node`].
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the required Python version is not available.
    /// Returns an error if the function cannot find the binary or start the [`Indexer`].
    /// Returns an error if the configuration is not valid or the [`Node`] is not ready.
    pub fn new_with_conf<N: Node>(node: &N, conf: &ElectrumxDConf) -> Result<Self, Error> {
        Self::validate_python_version()?;

        let electrumx_path = get_electrumx_path()?;
        Self::from_bin_with_conf(electrumx_path, node, conf)
    }

    /// Start the binary at [`Path`] with the default [`ElectrumxDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if `electrumx_bin` is not valid or the [`Node`] is not ready.
    /// Returns an error if the function cannot start the [`Indexer`].
    pub fn from_bin<P: AsRef<Path>, N: Node>(electrumx_bin: P, node: &N) -> Result<Self, Error> {
        Self::from_bin_with_conf(electrumx_bin, node, &ElectrumxDConf::default())
    }

    /// Start the binary at [`Path`] with the specified [`ElectrumxDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is not valid or the [`Node`] is not ready.
    /// Returns an error if directory creation or all start attempts fail.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>, N: Node>(
        electrumx_bin: P,
        node: &N,
        conf: &ElectrumxDConf,
    ) -> Result<Self, Error> {
        validate_backend::<N>()?;
        let node_args = node.get_config().as_ref();
        let configured_args = Self::configured_args(conf, node_args.network)?;

        let electrumx_bin = electrumx_bin.as_ref();
        if !electrumx_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: electrumx_bin.display().to_string(),
            });
        }
        if !electrumx_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: electrumx_bin.display().to_string(),
            });
        }

        Self::validate_node_args(node_args)?;
        let (_, credentials) = read_backend_cookie(node)?;
        ensure_backend_ready(node, node_args.network, Self::get_name())?;
        let node_rpc_socket = node.get_rpc_socket();
        let daemon_url = format!("http://{credentials}@{node_rpc_socket}");

        for _attempt in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-electrumx-",
            )?;

            let electrum_port = get_available_port();
            let electrum_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, electrum_port));

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, rpc_port));

            let services = format!("tcp://{},rpc://{}", electrum_socket, rpc_socket);
            let db_directory = working_directory.path().display().to_string();

            let mut args = configured_args.clone();
            args.extend(conf.raw_args.iter().cloned());
            args.extend([
                "--db-directory".to_string(),
                db_directory,
                "--daemon-url".to_string(),
                daemon_url.clone(),
                "--services".to_string(),
                services,
                "--peer-discovery".to_string(),
                "off".to_string(),
            ]);

            debug!(
                "Spawning {} [ELECTRUM_SOCKET={}, RPC_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                electrum_socket,
                rpc_socket,
                working_directory.path().display()
            );

            let mut command = Command::new(electrumx_bin);
            command.args(&args);

            let mut process = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "electrumx");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "electrumx");
            }

            sleep(SPAWN_INTERVAL);
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
                Self::wait_for_client(electrum_socket, &mut process, Duration::from_secs(15))
            {
                sleep(Duration::from_millis(200));

                debug!(
                    "Started {} [PID={}, ELECTRUM_SOCKET={}, RPC_SOCKET={}, DATADIR={}]",
                    Self::get_name(),
                    process.id(),
                    electrum_socket,
                    rpc_socket,
                    working_directory.path().display()
                );

                return Ok(Self {
                    process,
                    client,
                    working_directory,
                    config: conf.clone(),
                    electrum_socket,
                    rpc_socket,
                });
            }
            let _ = process.kill();
            let _ = process.wait();
        }

        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Tell `ElectrumX` to check its [`Node`] for new chain data.
    ///
    /// This function sends the local admin RPC command `reorg` with `count = 0`.
    /// The command starts the `ElectrumX` block processor but does not remove indexed blocks.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot connect, write, or read through the admin RPC
    /// socket. Returns an error if the socket returns an error response.
    pub fn trigger(&self) -> Result<(), Error> {
        self.trigger_reorg(0)
    }

    /// Start the local `ElectrumX` admin command `reorg` with `count`.
    fn trigger_reorg(&self, count: u32) -> Result<(), Error> {
        debug!(
            "{}: triggering daemon refresh rpc_socket={} reorg_count={}",
            Self::get_name(),
            self.rpc_socket,
            count
        );

        self.send_admin_rpc("reorg", &serde_json::json!({ "count": count }))?;

        debug!("{}: triggered daemon refresh", Self::get_name());

        Ok(())
    }

    /// Send a command to the local `ElectrumX` admin RPC socket.
    ///
    /// Return `false` if the server does not accept connections.
    fn send_admin_rpc(&self, method: &str, params: &serde_json::Value) -> Result<bool, Error> {
        send_admin_rpc_to(self.rpc_socket, method, params)
    }

    /// Stop the `ElectrumX` process and wait for it to exit.
    ///
    /// [`Drop`] stops the process without a call to this method.
    /// Call this method to get the exit status or confirm that the process has stopped.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot wait for the child process.
    pub fn stop(&mut self) -> Result<std::process::ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());
        if !matches!(
            self.send_admin_rpc("stop", &serde_json::json!({})),
            Ok(true)
        ) {
            if cfg!(target_os = "windows") {
                let _ = Command::new("taskkill")
                    .args(["/PID", &self.process.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            } else {
                let _ = self.process.kill();
            }
        }
        self.process.wait().map_err(Error::Io)
    }

    /// Return the operating system process ID of `ElectrumX`.
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", Self::get_name(), pid);

        pid
    }

    /// Return the data directory of [`ElectrumxD`].
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
    pub fn get_config(&self) -> &ElectrumxDConf {
        &self.config
    }

    /// Return a reference to the Electrum [`RawClient`] of [`ElectrumxD`].
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

    /// Return the admin RPC [`SocketAddr`] of the [`Indexer`].
    pub fn get_rpc_socket(&self) -> SocketAddr {
        debug!(
            "{}: got admin RPC socket at socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        self.rpc_socket
    }

    /// Poll until the Electrum header tip matches the tip of a [`Node`].
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

    /// Poll until the Electrum header tip of [`ElectrumxD`] reaches `exp_height`.
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
    /// If `timeout` is `None`, the function uses [`ELECTRUMX_INDEXING_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if Electrum subscription/history calls fail or the
    /// transaction does not appear before the timeout.
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

        let client = self.fresh_electrum_client()?;
        let (subscribed, initial_status) = match client.script_subscribe(spk) {
            Ok(status) => (true, status),
            Err(ElectrumError::AlreadySubscribed(_)) => (false, None),
            Err(err) => return Err(unresponsive_indexer(err).into()),
        };

        let timeout = timeout.unwrap_or(ELECTRUMX_INDEXING_TIMEOUT);
        let result = (|| {
            if initial_status.is_some() && Self::script_history_has_mempool_tx(&client, spk, txid)?
            {
                debug!(
                    "{}: found mempool transaction with txid={}",
                    Self::get_name(),
                    txid
                );

                return Ok(());
            }

            let start = Instant::now();
            while start.elapsed() < timeout {
                self.trigger_reorg(0)?;
                client.ping().or_else(empty_read_is_no_ping_response)?;

                if client
                    .script_pop(spk)
                    .or_else(empty_read_is_no_script_notification)?
                    .is_some()
                    && Self::script_history_has_mempool_tx(&client, spk, txid)?
                {
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
        })();

        if subscribed {
            let _ = client.script_unsubscribe(spk);
        }

        result
    }

    /// Validate raw arguments and create [`Indexer`] arguments.
    fn configured_args(conf: &ElectrumxDConf, network: Network) -> Result<Vec<String>, Error> {
        const OPTIONS: &[&str] = &[
            "coin",
            "daemon-url",
            "db-directory",
            "net",
            "peer-discovery",
            "services",
        ];
        const BOOLEAN_OPTIONS: &[&str] = &["peer-discovery"];

        if let Some(arg) = find_conflicting_argument(&conf.raw_args, OPTIONS, BOOLEAN_OPTIONS) {
            return Err(IndexerError::ConflictingArgument(arg).into());
        }

        let network = match network {
            Network::Bitcoin => "mainnet",
            Network::Testnet => "testnet",
            Network::Testnet4 => "testnet4",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
        };

        Ok(vec![
            "--coin".to_string(),
            conf.electrumx_args.coin.clone(),
            "--net".to_string(),
            network.to_string(),
        ])
    }

    /// Return whether the history of `spk` contains `txid` as an unconfirmed transaction.
    fn script_history_has_mempool_tx(
        client: &RawClient<ElectrumPlaintextStream>,
        spk: &Script,
        txid: Txid,
    ) -> Result<bool, Error> {
        client
            .script_get_history(spk)
            .map(|history| {
                let has_tx = history
                    .iter()
                    .any(|entry| entry.tx_hash == txid && entry.height == 0);

                debug!(
                    "{}: checked script mempool transaction with txid={} found={}",
                    Self::get_name(),
                    txid,
                    has_tx
                );

                has_tx
            })
            .map_err(|source| unresponsive_indexer(source).into())
    }

    /// Wait until the block header at `exp_height` matches `exp_hash`.
    fn wait_until_block(
        &self,
        exp_height: u32,
        exp_hash: Option<BlockHash>,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let client = self.get_electrum_client();

        let description = match exp_hash {
            Some(hash) => format!("block {exp_height} ({hash})"),
            None => format!("block {exp_height}"),
        };

        let timeout = timeout.unwrap_or(ELECTRUMX_INDEXING_TIMEOUT);
        debug!(
            "{}: waiting until indexed {} timeout={:?}",
            Self::get_name(),
            description,
            timeout
        );

        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger_reorg(0)?;

            let header = match client.block_header(
                usize::try_from(exp_height)
                    .map_err(|err| Error::UnexpectedResponse(err.to_string()))?,
            ) {
                Ok(header) => header,
                Err(err) if is_header_not_ready(&err) => {
                    sleep(2 * POLL_INTERVAL);
                    continue;
                }
                Err(err) => return Err(unresponsive_indexer(err).into()),
            };

            if exp_hash.is_none_or(|exp_hash| header.block_hash() == exp_hash) {
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

    /// Reject [`Node`] configurations that `ElectrumX` cannot index.
    fn validate_node_args(args: &NodeArgs) -> Result<(), Error> {
        if !args.txindex {
            return Err(IndexerError::InvalidConfiguration(
                "ElectrumX requires a backing node with transaction indexing enabled".to_string(),
            )
            .into());
        }
        Ok(())
    }

    /// Validate that the Python version required by the bundled `ElectrumX` launcher is available.
    ///
    /// Use `PYTHON` to select an interpreter.
    /// Without `PYTHON`, Unix uses `python3.10` and Windows uses `py -3.10`.
    /// Windows ARM64 uses `py -3.11`.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the selected command cannot start.
    /// It also returns this error if the `--version` check returns an error status.
    fn validate_python_version() -> Result<(), Error> {
        // Python version required by the bundled `ElectrumX` launcher.
        const PYTHON_VERSION: &str = "3.10";

        // Python version required by the bundled Windows ARM64 `ElectrumX` launcher.
        const PYTHON_VERSION_WINDOWS_ARM64: &str = "3.11";

        let mut python = if let Some(python) = env::var_os("PYTHON") {
            Command::new(python)
        } else if cfg!(target_os = "windows") {
            let mut python = Command::new("py");
            let version = if cfg!(target_arch = "aarch64") {
                PYTHON_VERSION_WINDOWS_ARM64
            } else {
                PYTHON_VERSION
            };
            python.arg(format!("-{version}"));
            python
        } else {
            Command::new(format!("python{PYTHON_VERSION}"))
        };
        let status = python
            .arg("--version")
            .output()
            .map_err(|e| {
                IndexerError::InvalidPython(format!("failed to run Python version check: {e}"))
            })?
            .status;
        if !status.success() {
            return Err(IndexerError::InvalidPython(format!(
                "Python version check failed with {status}"
            ))
            .into());
        }
        Ok(())
    }

    /// Create a temporary Electrum client for subscription wait functions.
    fn fresh_electrum_client(&self) -> Result<RawClient<ElectrumPlaintextStream>, Error> {
        RawClient::new(self.electrum_socket, Some(Duration::from_secs(5)), None)
            .map_err(|source| unresponsive_indexer(source).into())
    }

    /// Poll `server.ping` until it succeeds.
    fn wait_for_client(
        electrum_socket: SocketAddr,
        process: &mut Child,
        timeout: Duration,
    ) -> Result<RawClient<ElectrumPlaintextStream>, Error> {
        let start = Instant::now();
        let mut last_error = None;
        while start.elapsed() < timeout {
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => return Err(Error::ClientSetupTimeout),
                Ok(None) => {}
            }

            match RawClient::new(electrum_socket, Some(Duration::from_secs(5)), None) {
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

/// Send a command to an `ElectrumX` admin RPC socket.
fn send_admin_rpc_to(
    rpc_socket: SocketAddr,
    method: &str,
    params: &serde_json::Value,
) -> Result<bool, Error> {
    let mut stream = match TcpStream::connect_timeout(&rpc_socket, Duration::from_secs(1)) {
        Ok(stream) => stream,
        Err(err) if err.kind() == ErrorKind::ConnectionRefused => return Ok(false),
        Err(err) => return Err(Error::Io(err)),
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(Error::Io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(Error::Io)?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": method,
        "params": params,
    });
    writeln!(stream, "{request}").map_err(Error::Io)?;
    stream.flush().map_err(Error::Io)?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(Error::Io)?;
    let response: serde_json::Value = serde_json::from_str(&response)
        .map_err(|err| Error::UnexpectedResponse(err.to_string()))?;

    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        return Err(Error::UnexpectedResponse(format!(
            "ElectrumX admin method `{method}` failed: {error}"
        )));
    }

    Ok(true)
}

impl Drop for ElectrumxD {
    /// Stop the `electrumx` process and wait for it to exit.
    ///
    /// Ignore errors to prevent a panic in [`Drop`].
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Interpret an empty nonblocking subscription read as no script notification.
fn empty_read_is_no_script_notification(err: ElectrumError) -> Result<Option<ScriptStatus>, Error> {
    if is_empty_subscription_read(&err) {
        return Ok(None);
    }

    Err(unresponsive_indexer(err).into())
}

/// Interpret an empty nonblocking read during a wait loop as an incomplete wait.
fn empty_read_is_no_ping_response(err: ElectrumError) -> Result<(), Error> {
    if is_empty_subscription_read(&err) {
        return Ok(());
    }

    Err(unresponsive_indexer(err).into())
}

/// Return whether an Electrum client error means that the subscribed socket has no queued message.
fn is_empty_subscription_read(err: &ElectrumError) -> bool {
    matches!(
        err,
        ElectrumError::IOError(io_err)
            if matches!(
                io_err.kind(),
                ErrorKind::WouldBlock | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe
            )
    ) || matches!(
        err,
        ElectrumError::SharedIOError(io_err)
            if matches!(
                io_err.kind(),
                ErrorKind::WouldBlock | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe
            )
    )
}

/// Return whether a block header request failed because the [`Indexer`] has not reached that
/// header.
fn is_header_not_ready(err: &ElectrumError) -> bool {
    is_empty_subscription_read(err) || matches!(err, ElectrumError::Protocol(_))
}

#[cfg(all(test, halfin_indexer))]
mod test;
