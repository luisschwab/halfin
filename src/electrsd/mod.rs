// SPDX-License-Identifier: MIT OR Apache-2.0

//! # ElectrsD: spawn and interact with an `electrs` process
//!
//! A utility crate for spinning up `electrs` processes connected to a local
//! [`BitcoinD`] process in **regtest**, useful for integration testing Electrum
//! consumers against a Bitcoin regtest chain.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use halfin::bitcoind::BitcoinD;
//! use halfin::electrsd::ElectrsD;
//!
//! let bitcoind = BitcoinD::new().unwrap();
//! bitcoind.generate(10).unwrap();
//!
//! let electrs = ElectrsD::new(&bitcoind).unwrap();
//! electrs.wait_until_caught_up(&bitcoind, None).unwrap();
//! ```
//!
//! ## Directory Handling
//!
//! By default each [`ElectrsD`] instance uses a temporary directory that is
//! cleaned up when the instance is dropped. Pass a `staticdir` in
//! [`ElectrsDConf`] to keep data between runs.

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::Txid;
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
use electrum_client::HeaderNotification;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use tracing::debug;

use crate::BitcoinD;
use crate::DataDir;
use crate::Error;
use crate::IPV4_LOCALHOST;
use crate::NODE_BUILDING_INTERVAL;
use crate::NODE_BUILDING_MAX_RETRIES;
use crate::Node;
use crate::POLL_INTERVAL;
use crate::get_available_port;
use crate::pipe_to_tracing;

mod versions;

/// The default timeout for [`ElectrsD`] indexing helpers.
pub const ELECTRS_INDEXING_TIMEOUT: Duration = Duration::from_secs(30);

/// Return the path to the downloaded `electrs` binary.
///
/// The path is resolved at compile time from the `HALFIN_ELECTRS_PATH`
/// environment variable, which is set by `build.rs` after reading and
/// extracting the local archive.
pub fn get_electrs_path() -> Result<PathBuf, Error> {
    let bin_name = ElectrsD::get_bin_name().to_string();
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_ELECTRS_PATH").unwrap_or(""));

    // Add the `.exe` suffix on Windows.
    #[cfg(target_os = "windows")]
    if bin_path.extension().is_none() {
        bin_path.set_extension("exe");
    }

    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

/// Configuration for an [`ElectrsD`] instance.
///
/// Build one explicitly or call [`ElectrsDConf::default`] for sensible regtest
/// defaults.
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
pub struct ElectrsDConf<'a> {
    /// Extra CLI arguments forwarded verbatim to the `electrs` process.
    pub args: Vec<&'a str>,

    /// Bitcoin network passed to `electrs`. Defaults to `regtest`.
    pub network: &'a str,

    /// Whether to expose `electrs`' HTTP/Esplora endpoint.
    pub http_enabled: bool,

    /// Root directory under which a fresh temporary working directory is
    /// created for each instance. Falls back to the `TEMPDIR_ROOT`
    /// environment variable, then the system temp dir.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory. The directory is created if it does not
    /// exist. Data survives [`Drop`]; the process is stopped but files are
    /// kept so you can inspect or reuse them.
    pub staticdir: Option<PathBuf>,

    /// How many times to retry spawning `electrs` before giving up.
    ///
    /// Each attempt picks fresh random ports, so transient port-collision
    /// errors are automatically recovered from. Defaults to [`NODE_BUILDING_MAX_RETRIES`].
    pub max_retries: u8,
}

impl Default for ElectrsDConf<'_> {
    fn default() -> Self {
        ElectrsDConf {
            args: vec![],
            network: "regtest",
            http_enabled: false,
            tmpdir: None,
            staticdir: None,
            max_retries: NODE_BUILDING_MAX_RETRIES,
        }
    }
}

/// A running `electrs` regtest indexer.
///
/// The indexer is started in [`ElectrsD::from_bin`] (or one of its siblings),
/// connected to the supplied [`BitcoinD`], and stopped when this value is
/// dropped.
///
/// # Networking
///
/// The Electrum RPC, monitoring, and optional HTTP ports are chosen from the
/// OS's ephemeral range at startup. Use [`electrum_socket`](ElectrsD::electrum_socket),
/// [`monitoring_socket`](ElectrsD::monitoring_socket), and
/// [`esplora_socket`](ElectrsD::esplora_socket) to discover them after
/// construction.
pub struct ElectrsD {
    /// Handle to the spawned `electrs` child process.
    process: Child,
    /// Plaintext Electrum client connected to `electrs`.
    pub client: RawClient<ElectrumPlaintextStream>,
    /// Owns (and optionally cleans up) the indexer's data directory.
    working_directory: DataDir,
    /// Address the Electrum RPC server is bound to.
    electrum_socket: SocketAddr,
    /// Address the monitoring server is bound to.
    monitoring_socket: SocketAddr,
    /// Address the optional HTTP/Esplora server is bound to.
    esplora_socket: Option<SocketAddr>,
}

#[rustfmt::skip]
impl ElectrsD {
    /// [`ElectrsD`]'s human-readable name.
    pub fn get_name() -> &'static str { "ElectrsD" }

    /// [`ElectrsD`]'s binary name.
    pub fn get_bin_name() -> &'static str { "electrs_v_0_11_1" }
}

impl ElectrsD {
    // ----> ELECTRS

    /// Start an [`ElectrsD`] indexer using the binary located by [`get_electrs_path`], with the default [`ElectrsDConf`].
    ///
    /// The indexer connects to the supplied [`BitcoinD`] process.
    pub fn new(bitcoind: &BitcoinD) -> Result<ElectrsD, Error> {
        ElectrsD::from_bin(get_electrs_path()?, bitcoind)
    }

    /// Start an [`ElectrsD`] indexer using the binary located by [`get_electrs_path`], with a custom [`ElectrsDConf`].
    ///
    /// The indexer connects to the supplied [`BitcoinD`] process.
    pub fn new_with_conf(bitcoind: &BitcoinD, conf: &ElectrsDConf) -> Result<ElectrsD, Error> {
        ElectrsD::from_bin_with_conf(get_electrs_path()?, bitcoind, conf)
    }

    /// Create an [`ElectrsD`] instance running the binary at [`Path`] with the default [`ElectrsDConf`].
    pub fn from_bin<P: AsRef<Path>>(
        electrs_bin: P,
        bitcoind: &BitcoinD,
    ) -> Result<ElectrsD, Error> {
        ElectrsD::from_bin_with_conf(electrs_bin, bitcoind, &ElectrsDConf::default())
    }

    /// Create an [`ElectrsD`] instance running the binary at [`Path`] with a custom [`ElectrsDConf`].
    ///
    /// The method retries up to [`ElectrsDConf::max_retries`] times. On each
    /// attempt it:
    ///
    /// 1. Picks fresh ephemeral Electrum, monitoring, and optional HTTP ports.
    /// 2. Spawns `electrs` pointed at the supplied [`BitcoinD`]'s RPC and P2P sockets.
    /// 3. Waits for the Electrum RPC server to become responsive (up to 10 s).
    ///
    /// Returns an error if all attempts are exhausted.
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        electrs_bin: P,
        bitcoind: &BitcoinD,
        conf: &ElectrsDConf,
    ) -> Result<ElectrsD, Error> {
        // Validate the `electrs_bin` path.
        let electrs_bin = electrs_bin.as_ref();
        // The path must be absolute.
        if !electrs_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: ElectrsD::get_bin_name().to_string(),
                path: electrs_bin.display().to_string(),
            });
        }
        // The path must be a file.
        if !electrs_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: ElectrsD::get_bin_name().to_string(),
                path: electrs_bin.display().to_string(),
            });
        }

        Self::ensure_bitcoind_ready(bitcoind)?;

        for _attempt in 0..=conf.max_retries {
            let working_directory = Self::init_work_dir(conf)?;

            let electrum_port = get_available_port();
            let electrum_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, electrum_port));

            let monitoring_port = get_available_port();
            let monitoring_socket =
                SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, monitoring_port));

            let esplora_socket = conf
                .http_enabled
                .then(|| SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, get_available_port())));

            let mut args: Vec<String> = conf.args.iter().map(|arg| arg.to_string()).collect();
            args.extend([
                "--db-dir".to_string(),
                working_directory.path().display().to_string(),
                "--network".to_string(),
                conf.network.to_string(),
                "--daemon-rpc-addr".to_string(),
                bitcoind.rpc_socket().to_string(),
                "--daemon-p2p-addr".to_string(),
                bitcoind.get_p2p_socket().to_string(),
                "--electrum-rpc-addr".to_string(),
                electrum_socket.to_string(),
                "--monitoring-addr".to_string(),
                monitoring_socket.to_string(),
                "--cookie-file".to_string(),
                bitcoind.cookie_file().display().to_string(),
            ]);
            if let Some(esplora_socket) = esplora_socket {
                args.extend(["--http-addr".to_string(), esplora_socket.to_string()]);
            }

            debug!(
                "Spawning {} [ELECTRUM_SOCKET={}, MONITORING_SOCKET={}, ESPLORA_SOCKET={:?}, DATADIR={}]",
                ElectrsD::get_name(),
                electrum_socket,
                monitoring_socket,
                esplora_socket,
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
            thread::sleep(NODE_BUILDING_INTERVAL);

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    debug!(
                        "{} exited immediately, retrying with fresh ports",
                        ElectrsD::get_name()
                    );
                    let _ = process.kill();
                    continue;
                }
                Ok(None) => {}
            }

            match Self::wait_for_client(electrum_socket, &mut process, Duration::from_secs(10)) {
                Ok(client) => {
                    sleep(Duration::from_millis(200));

                    debug!(
                        "Started {} [PID={}, ELECTRUM_SOCKET={}, MONITORING_SOCKET={}, ESPLORA_SOCKET={:?}, DATADIR={}]",
                        ElectrsD::get_name(),
                        process.id(),
                        electrum_socket,
                        monitoring_socket,
                        esplora_socket,
                        working_directory.path().display()
                    );

                    return Ok(ElectrsD {
                        process,
                        client,
                        working_directory,
                        electrum_socket,
                        monitoring_socket,
                        esplora_socket,
                    });
                }
                Err(_) => {
                    let _ = process.kill();
                    continue;
                }
            }
        }

        Err(Error::ExhaustedNodeBuildingRetries(conf.max_retries))
    }

    /// Send `SIGUSR1` to trigger a rescan on Unix-derived platforms.
    ///
    /// This is a no-op on Windows.
    #[cfg(not(target_os = "windows"))]
    pub fn trigger(&self) -> Result<(), Error> {
        let status = Command::new("kill")
            .arg("-USR1")
            .arg(self.process.id().to_string())
            .status()
            .map_err(Error::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse(format!(
                "failed to trigger electrs rescan with exit status={status}"
            )))
        }
    }

    /// No-op rescan trigger on Windows.
    #[cfg(target_os = "windows")]
    pub fn trigger(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Kill the `electrs` process and wait for it to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically. It is provided for cases where you
    /// need the exit status or want to ensure the indexer has fully shut down
    /// before proceeding.
    pub fn stop(&mut self) -> Result<std::process::ExitStatus, Error> {
        debug!(
            "Stopping {} [PID={}]",
            ElectrsD::get_name(),
            self.process.id()
        );
        let _ = self.process.kill();
        self.process.wait().map_err(Error::Io)
    }

    /// Return the OS process ID of the running `electrs` process.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }

    /// Get [`ElectrsD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }

    /// Get a reference to [`ElectrsD`]'s Electrum [`RawClient`].
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        &self.client
    }

    /// Return the Electrum RPC [`SocketAddr`] the indexer is listening on.
    pub fn electrum_socket(&self) -> SocketAddr {
        self.electrum_socket
    }

    /// Return the Electrum RPC URL for the indexer.
    pub fn electrum_url(&self) -> String {
        self.electrum_socket.to_string()
    }

    /// Return the monitoring [`SocketAddr`] the indexer is listening on.
    pub fn monitoring_socket(&self) -> SocketAddr {
        self.monitoring_socket
    }

    /// Return the optional HTTP/Esplora [`SocketAddr`] the indexer is listening on.
    pub fn esplora_socket(&self) -> Option<SocketAddr> {
        self.esplora_socket
    }

    /// Return the optional HTTP/Esplora URL for the indexer.
    pub fn esplora_url(&self) -> Option<String> {
        self.esplora_socket
            .map(|esplora_socket| format!("http://{esplora_socket}"))
    }

    /// Poll until this [`ElectrsD`]'s Electrum header tip matches [`BitcoinD`]'s tip.
    ///
    /// Both the tip height and block hash are verified. Pass `None` to use
    /// [`ELECTRS_INDEXING_TIMEOUT`].
    pub fn wait_until_caught_up(
        &self,
        bitcoind: &BitcoinD,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let height = bitcoind.get_chain_tip()?;
        let hash = bitcoind.get_block_hash(height)?;
        self.wait_until_block(height, Some(hash), timeout)
    }

    /// Poll until this [`ElectrsD`]'s Electrum header tip reaches `exp_height`.
    ///
    /// The block hash at `exp_height` is verified against `exp_hash`.
    /// Pass `None` to use [`ELECTRS_INDEXING_TIMEOUT`].
    pub fn wait_until_tip(
        &self,
        exp_height: u32,
        exp_hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        self.wait_until_block(exp_height, Some(exp_hash), timeout)
    }

    /// Poll until `txid` appears as an unconfirmed transaction for `script_pubkey`.
    ///
    /// Pass `None` to use [`ELECTRS_INDEXING_TIMEOUT`].
    pub fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let (subscribed, initial_status) = match self.client.script_subscribe(spk) {
            Ok(status) => (true, status),
            Err(ElectrumError::AlreadySubscribed(_)) => (false, None),
            Err(err) => return Err(Error::UnresponsiveElectrsD(err)),
        };
        let description = format!("mempool transaction {txid}");
        let timeout = timeout.unwrap_or(ELECTRS_INDEXING_TIMEOUT);
        let result = (|| {
            if initial_status.is_some() && self.script_history_has_mempool_tx(spk, txid)? {
                return Ok(());
            }

            let start = Instant::now();
            while start.elapsed() < timeout {
                self.trigger()?;
                self.client.ping().map_err(Error::UnresponsiveElectrsD)?;

                if self
                    .client
                    .script_pop(spk)
                    .map_err(Error::UnresponsiveElectrsD)?
                    .is_some()
                    && self.script_history_has_mempool_tx(spk, txid)?
                {
                    return Ok(());
                }

                thread::sleep(2 * POLL_INTERVAL);
            }

            Err(Error::ElectrsDIndexTimeout((description, timeout)))
        })();

        if subscribed {
            let _ = self.client.script_unsubscribe(spk);
        }

        result
    }

    // ----> INTERNAL

    /// Return whether `script_pubkey` history contains `txid` as an unconfirmed transaction.
    fn script_history_has_mempool_tx(&self, spk: &Script, txid: Txid) -> Result<bool, Error> {
        self.client
            .script_get_history(spk)
            .map(|history| {
                history
                    .iter()
                    .any(|entry| entry.tx_hash == txid && entry.height == 0)
            })
            .map_err(Error::UnresponsiveElectrsD)
    }

    /// Wait for an Electrum block-header notification proving `exp_height`/`exp_hash` is indexed.
    ///
    /// Electrs sends header notifications only after its confirmed script
    /// histories are current for the notified tip, so this waits on
    /// notifications instead of polling block headers directly.
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
                .map_err(Error::UnresponsiveElectrsD)?,
        );

        let description = match exp_hash {
            Some(hash) => format!("block {exp_height} ({hash})"),
            None => format!("block {exp_height}"),
        };

        let timeout = timeout.unwrap_or(ELECTRS_INDEXING_TIMEOUT);
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.trigger()?;
            client.ping().map_err(Error::UnresponsiveElectrsD)?;

            let notification = match next_notification.take() {
                Some(notification) => Some(notification),
                None => client
                    .block_headers_pop()
                    .map_err(Error::UnresponsiveElectrsD)?,
            };
            let Some(notification) = notification else {
                thread::sleep(2 * POLL_INTERVAL);
                continue;
            };

            if electrs_header_matches(client, notification, exp_height, exp_hash)? {
                return Ok(());
            }

            thread::sleep(2 * POLL_INTERVAL);
        }

        Err(Error::ElectrsDIndexTimeout((description, timeout)))
    }

    /// Ensure the backing [`BitcoinD`] has left initial block download.
    fn ensure_bitcoind_ready(bitcoind: &BitcoinD) -> Result<(), Error> {
        let blockchain_info = bitcoind.call("getblockchaininfo", &[])?;
        let initial_block_download = blockchain_info
            .get("initialblockdownload")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if initial_block_download {
            let _ = bitcoind.generate(1)?;
        }
        Ok(())
    }

    /// Resolve and create the working directory according to `conf`.
    ///
    /// Precedence: `conf.tmpdir` → `TEMPDIR_ROOT` env var → system temp.
    /// If `conf.staticdir` is set the directory is created but never cleaned
    /// up automatically.
    fn init_work_dir(conf: &ElectrsDConf) -> Result<DataDir, Error> {
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
                    .prefix("halfin-electrs-")
                    .tempdir_in(tmpdir)
                    .map_err(Error::Io)?,
            ),
            (None, None) => DataDir::Temporary(
                tempfile::Builder::new()
                    .prefix("halfin-electrs-")
                    .tempdir()
                    .map_err(Error::Io)?,
            ),
        };
        Ok(work_dir)
    }

    /// Poll `server.ping` until it succeeds, building
    /// and returning the Electrum client on success.
    ///
    /// Returns `Err` if the indexer is not responsive within `timeout`.
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
                    return Err(Error::RpcClientSetupTimeout);
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
            thread::sleep(Duration::from_millis(200));
        }

        Err(last_error
            .map(Error::UnresponsiveElectrsD)
            .unwrap_or(Error::RpcClientSetupTimeout))
    }
}

impl Drop for ElectrsD {
    /// Kills the `electrs` process.
    ///
    /// Errors from `kill` are silently discarded so that [`Drop`] never panics.
    fn drop(&mut self) {
        debug!(
            "Dropping {} [PID={}]",
            ElectrsD::get_name(),
            self.process.id()
        );
        let _ = self.process.kill();
    }
}

/// Check whether an Electrum header notification proves ElectrsD has indexed `exp_height`.
///
/// If the notification has advanced past `exp_height`, fetch the header at
/// `exp_height` explicitly so `exp_hash` can still be verified.
fn electrs_header_matches(
    client: &RawClient<ElectrumPlaintextStream>,
    notification: HeaderNotification,
    exp_height: u32,
    exp_hash: Option<BlockHash>,
) -> Result<bool, Error> {
    let notification_height = u32::try_from(notification.height)
        .map_err(|err| Error::UnexpectedResponse(err.to_string()))?;

    if notification_height < exp_height {
        return Ok(false);
    }

    let header = if notification_height == exp_height {
        notification.header
    } else {
        client
            .block_header(
                usize::try_from(exp_height)
                    .map_err(|err| Error::UnexpectedResponse(err.to_string()))?,
            )
            .map_err(Error::UnresponsiveElectrsD)?
    };

    Ok(exp_hash
        .map(|exp_hash| header.block_hash() == exp_hash)
        .unwrap_or(true))
}
