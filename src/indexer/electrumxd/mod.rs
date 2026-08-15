// SPDX-License-Identifier: MIT OR Apache-2.0

//! # `ElectrumxD`: spawn and interact with an `electrumx` indexer process
//!
//! A utility crate for spinning up `ElectrumX` processes connected to a local
//! [`Node`] process.

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

/// Wrap an Electrum client failure with indexer context.
fn unresponsive_indexer(source: ElectrumError) -> IndexerError {
    IndexerError::UnresponsiveIndexer {
        indexer: ElectrumxD::get_name(),
        source,
    }
}

/// Return the path to the locally extracted `ElectrumX` launcher.
///
/// The path is resolved at compile time from the `HALFIN_ELECTRUMX_PATH`
/// environment variable, which is set by `build.rs` after extracting the local
/// archive produced by `contrib/compile_electrumx`.
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
pub struct ElectrumxDConf {
    /// Arguments specific to `ElectrumX`.
    pub electrumx_args: ElectrumxDArgs,

    /// Extra CLI arguments forwarded verbatim to the `ElectrumX` launcher.
    ///
    /// Raw arguments must not configure an option represented by
    /// [`electrumx_args`](Self::electrumx_args), or owned
    /// dynamically by `halfin`. Such duplicates return
    /// [`IndexerError::ConflictingArgument`].
    pub raw_args: Vec<String>,

    /// Root directory under which a fresh temporary working directory is
    /// created for each instance. Falls back to the `TEMPDIR_ROOT`
    /// environment variable, then the system temp dir.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory. The directory is created if it does not
    /// exist. Data survives [`Drop`]; the process is stopped but files are
    /// kept so you can inspect or reuse them.
    pub staticdir: Option<PathBuf>,

    /// How many times to retry spawning `ElectrumX` before giving up.
    ///
    /// Each attempt picks fresh random ports, so transient port-collision
    /// errors are automatically recovered from. Defaults to [`SPAWN_ATTEMPTS`].
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

/// A running `ElectrumX` indexer.
#[derive(Debug)]
pub struct ElectrumxD {
    /// Handle to the spawned `ElectrumX` child process.
    process: Child,

    /// Plaintext Electrum client connected to `ElectrumX`.
    pub client: RawClient<ElectrumPlaintextStream>,

    /// Owns the indexer's data directory.
    working_directory: DataDir,

    /// Complete configuration used to start the indexer.
    config: ElectrumxDConf,

    /// Address the Electrum RPC server is bound to.
    electrum_socket: SocketAddr,

    /// Address the admin RPC server is bound to.
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
    /// [`ElectrumxD`]'s human-readable name.
    pub fn get_name() -> &'static str { versions::ELECTRUMX_NAME }

    /// [`ElectrumxD`]'s binary name.
    pub fn get_bin_name() -> &'static str { versions::ELECTRUMX_BIN_NAME }
}

impl ElectrumxD {
    /// Start an [`ElectrumxD`] indexer using the binary located by [`get_electrumx_path`], with the
    /// default [`ElectrumxDConf`].
    ///
    /// The indexer connects to the supplied [`Node`].
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the bundled launcher's Python version is
    /// unavailable. Returns an error if the binary cannot be located, the node is not ready, or
    /// the indexer cannot be started.
    pub fn new<N: Node>(node: &N) -> Result<Self, Error> {
        Self::new_with_conf(node, &ElectrumxDConf::default())
    }

    /// Start an [`ElectrumxD`] indexer using the binary located by [`get_electrumx_path`], with a
    /// custom [`ElectrumxDConf`].
    ///
    /// The indexer connects to the supplied [`Node`].
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the bundled launcher's Python version is
    /// unavailable. Returns an error if the binary cannot be located, the configuration is
    /// invalid, the node is not ready, or the indexer cannot be started.
    pub fn new_with_conf<N: Node>(node: &N, conf: &ElectrumxDConf) -> Result<Self, Error> {
        Self::validate_python_version()?;

        let electrumx_path = get_electrumx_path()?;
        Self::from_bin_with_conf(electrumx_path, node, conf)
    }

    /// Create an [`ElectrumxD`] instance running the binary at [`Path`] with the default
    /// [`ElectrumxDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if `electrumx_bin` is invalid, the node is not ready,
    /// or the indexer cannot be started.
    pub fn from_bin<P: AsRef<Path>, N: Node>(electrumx_bin: P, node: &N) -> Result<Self, Error> {
        Self::from_bin_with_conf(electrumx_bin, node, &ElectrumxDConf::default())
    }

    /// Create an [`ElectrumxD`] instance running the binary at [`Path`] with a custom
    /// [`ElectrumxDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is invalid, the backing [`Node`]
    /// is not ready, the working directory cannot be created, or all attempts are exhausted.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>, N: Node>(
        electrumx_bin: P,
        node: &N,
        conf: &ElectrumxDConf,
    ) -> Result<Self, Error> {
        validate_backend::<N>()?;
        let node_args = *node.get_config().as_ref();
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

    /// Trigger `ElectrumX` to check the backing daemon for updated chain state.
    ///
    /// This sends the local admin RPC command `reorg` with `count = 0`. In
    /// `ElectrumX` this wakes the block processor without intentionally backing
    /// up indexed blocks.
    ///
    /// # Errors
    ///
    /// Returns an error if the admin RPC socket cannot be reached, written to,
    /// read from, or returns an error response.
    pub fn trigger(&self) -> Result<(), Error> {
        self.trigger_reorg(0)
    }

    /// Trigger `ElectrumX`'s local admin `reorg` command with `count`.
    fn trigger_reorg(&self, count: u32) -> Result<(), Error> {
        debug!(
            "{}: triggering daemon refresh rpc_socket={} reorg_count={}",
            Self::get_name(),
            self.rpc_socket,
            count
        );

        let mut stream = match TcpStream::connect_timeout(&self.rpc_socket, Duration::from_secs(1))
        {
            Ok(stream) => stream,
            Err(err) if err.kind() == ErrorKind::ConnectionRefused => return Ok(()),
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
            "method": "reorg",
            "params": {
                "count": count
            }
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
                "failed to trigger ElectrumX refresh: {error}"
            )));
        }

        debug!("{}: triggered daemon refresh", Self::get_name());

        Ok(())
    }

    /// Kill the `ElectrumX` process and wait for it to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically. It is provided for cases where you
    /// need the exit status or want to ensure the indexer has fully shut down
    /// before proceeding.
    ///
    /// # Errors
    ///
    /// Returns an error if the child process cannot be waited on.
    pub fn stop(&mut self) -> Result<std::process::ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());
        let _ = self.process.kill();
        self.process.wait().map_err(Error::Io)
    }

    /// Return the OS process ID of the running `ElectrumX` process.
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", Self::get_name(), pid);

        pid
    }

    /// Get [`ElectrumxD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        let working_directory = self.working_directory.path();

        debug!(
            "{}: got working directory at path={}",
            Self::get_name(),
            working_directory.display()
        );

        working_directory
    }

    /// Return the complete configuration used to start this indexer.
    pub fn get_config(&self) -> &ElectrumxDConf {
        &self.config
    }

    /// Get a reference to [`ElectrumxD`]'s Electrum [`RawClient`].
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        debug!(
            "{}: got electrum client for socket={}",
            Self::get_name(),
            self.electrum_socket
        );

        &self.client
    }

    /// Return the Electrum RPC [`SocketAddr`] the indexer is listening on.
    pub fn get_electrum_socket(&self) -> SocketAddr {
        debug!(
            "{}: got electrum socket at socket={}",
            Self::get_name(),
            self.electrum_socket
        );

        self.electrum_socket
    }

    /// Return the Electrum RPC URL for the indexer.
    pub fn get_electrum_url(&self) -> String {
        let electrum_url = self.electrum_socket.to_string();

        debug!(
            "{}: got electrum url at url={}",
            Self::get_name(),
            electrum_url
        );

        electrum_url
    }

    /// Return the admin RPC [`SocketAddr`] the indexer is listening on.
    pub fn get_rpc_socket(&self) -> SocketAddr {
        debug!(
            "{}: got admin RPC socket at socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        self.rpc_socket
    }

    /// Poll until this [`ElectrumxD`]'s Electrum header tip matches a [`Node`]'s tip.
    ///
    /// # Errors
    ///
    /// Returns an error if the backing node cannot be queried or the indexer
    /// does not catch up before the timeout.
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

    /// Poll until this [`ElectrumxD`]'s Electrum header tip reaches `exp_height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the indexer cannot be queried or does not reach the
    /// expected tip before the timeout.
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

    /// Poll until a transaction with [`Txid`]=`txid` appears as an unconfirmed transaction for
    /// `spk`.
    ///
    /// If `timeout` is `None`, the default [`ELECTRUMX_INDEXING_TIMEOUT`] will be used.
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

    /// Render indexer-owned arguments after validating raw arguments.
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

    /// Return whether this `spk`'s history contains `txid` as an unconfirmed transaction.
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

    /// Reject node configurations that `ElectrumX` cannot index.
    fn validate_node_args(args: NodeArgs) -> Result<(), Error> {
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
    /// The `PYTHON` environment variable selects an explicit interpreter. Otherwise, Unix uses
    /// `python3.10`, Windows uses `py -3.10`, and Windows ARM64 uses `py -3.11`.
    ///
    /// # Errors
    ///
    /// Returns [`IndexerError::InvalidPython`] if the selected command cannot be started or its
    /// `--version` check exits unsuccessfully.
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
        let status = python.arg("--version").status().map_err(|e| {
            IndexerError::InvalidPython(format!("failed to run Python version check: {e}"))
        })?;
        if !status.success() {
            return Err(IndexerError::InvalidPython(format!(
                "Python version check failed with {status}"
            ))
            .into());
        }
        Ok(())
    }

    /// Build a short-lived Electrum client for subscription wait helpers.
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

impl Drop for ElectrumxD {
    /// Kills the `electrumx` process and waits for it to exit.
    ///
    /// Errors from `kill` and `wait` are silently discarded so that [`Drop`]
    /// never panics.
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            Self::get_name(),
            self.process.id()
        );

        if cfg!(target_os = "windows") {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.process.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        } else {
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
    }
}

/// Treat a nonblocking empty subscription read as "no script notification yet".
fn empty_read_is_no_script_notification(err: ElectrumError) -> Result<Option<ScriptStatus>, Error> {
    if is_empty_subscription_read(&err) {
        return Ok(None);
    }

    Err(unresponsive_indexer(err).into())
}

/// Treat a nonblocking empty read during a wait-loop ping as "still waiting".
fn empty_read_is_no_ping_response(err: ElectrumError) -> Result<(), Error> {
    if is_empty_subscription_read(&err) {
        return Ok(());
    }

    Err(unresponsive_indexer(err).into())
}

/// Return whether an Electrum client error means the subscribed socket had no queued message.
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

/// Return whether an Electrum block-header lookup failed because the indexer has not reached it
/// yet.
fn is_header_not_ready(err: &ElectrumError) -> bool {
    is_empty_subscription_read(err) || matches!(err, ElectrumError::Protocol(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_defaults() {
        let conf = ElectrumxDConf::default();

        assert_eq!(conf.electrumx_args.coin, "Bitcoin");
        assert!(conf.raw_args.is_empty());
        assert_eq!(conf.max_retries, SPAWN_ATTEMPTS);
        assert_eq!(
            ElectrumxD::configured_args(&conf, Network::Regtest).unwrap(),
            ["--coin", "Bitcoin", "--net", "regtest"]
        );
    }

    #[test]
    fn renders_every_network() {
        let cases = [
            (Network::Bitcoin, "mainnet"),
            (Network::Testnet, "testnet"),
            (Network::Testnet4, "testnet4"),
            (Network::Signet, "signet"),
            (Network::Regtest, "regtest"),
        ];

        for (network, expected) in cases {
            let conf = ElectrumxDConf::default();

            assert_eq!(
                ElectrumxD::configured_args(&conf, network).unwrap(),
                ["--coin", "Bitcoin", "--net", expected]
            );
        }
    }

    #[test]
    fn renders_coin() {
        let conf = ElectrumxDConf {
            electrumx_args: ElectrumxDArgs {
                coin: "Namecoin".to_string(),
            },
            ..ElectrumxDConf::default()
        };

        assert_eq!(
            ElectrumxD::configured_args(&conf, Network::Regtest).unwrap(),
            ["--coin", "Namecoin", "--net", "regtest"]
        );
    }

    #[test]
    fn rejects_owned_raw_arguments() {
        let cases = [
            "--coin",
            "--coin=Bitcoin",
            "--daemon-url",
            "--daemon-url=http://user:pass@127.0.0.1:1",
            "--db-directory",
            "--db-directory=/tmp/electrumx",
            "--net",
            "--net=testnet",
            "--peer-discovery",
            "--peer-discovery=on",
            "--no-peer-discovery",
            "--nopeer-discovery",
            "--services",
            "--services=tcp://127.0.0.1:50001",
        ];

        for arg in cases {
            let conf = ElectrumxDConf {
                raw_args: vec![arg.to_string()],
                ..ElectrumxDConf::default()
            };

            assert!(matches!(
                ElectrumxD::configured_args(&conf, Network::Regtest),
                Err(Error::Indexer(IndexerError::ConflictingArgument(conflict))) if conflict == arg
            ));
        }
    }

    #[test]
    fn accepts_unmodeled_raw_arguments() {
        let conf = ElectrumxDConf {
            raw_args: vec![
                "--log-level=debug".to_string(),
                "--cache-mb=512".to_string(),
            ],
            ..ElectrumxDConf::default()
        };

        assert!(ElectrumxD::configured_args(&conf, Network::Regtest).is_ok());
    }
}
