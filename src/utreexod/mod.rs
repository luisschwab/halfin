// SPDX-License-Identifier: MIT OR Apache-2.0

//! # `UtreexoD`: spawn and interact with a `utreexod` process
//!
//! A utility for spinning up `utreexod` processes in **regtest**,
//! useful for integration testing Bitcoin applications that rely on
//! utreexo-based compact state.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use halfin::utreexod::UtreexoD;
//!
//! // Start a node with default configuration
//! let node = UtreexoD::new().unwrap();
//! ```

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

use corepc_client::bitcoin::BlockHash;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v17::AddNodeCommand;
use corepc_client::client_sync::v17::Client;
use tracing::debug;

use crate::CONNECTION_INTERVAL;
use crate::CONNECTION_TIMEOUT;
use crate::DataDir;
use crate::Error;
use crate::IPV4_LOCALHOST;
use crate::NODE_BUILDING_ATTEMPTS;
use crate::NODE_BUILDING_INTERVAL;
use crate::Node;
use crate::POLL_INTERVAL;
use crate::WAIT_TIMEOUT;
use crate::get_available_port;
use crate::pipe_to_tracing;

/// Bundled `utreexod` version metadata.
mod versions;

/// Username for RPC authentication.
const RPC_USER: &str = "halfin";

/// Password for RPC authentication.
const RPC_PASS: &str = "halfin";

/// Return the path to the downloaded `utreexod` binary.
///
/// The path is resolved at compile time from the `HALFIN_UTREEXOD_PATH`
/// environment variable, which is set by `build.rs` after downloading
/// and extracting the binary.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the compiled-in binary path does not exist.
pub fn get_utreexod_path() -> Result<PathBuf, Error> {
    let bin_name = UtreexoD::get_bin_name().to_string();
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_UTREEXOD_PATH").unwrap_or(""));

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

/// Configuration for a [`UtreexoD`] instance.
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
pub struct UtreexoDConf<'a> {
    /// Extra CLI arguments forwarded verbatim to the `utreexod` process.
    ///
    /// The defaults (`--regtest`, `--notls`, `--nodnsseed`, `--noassumeutreexo`, `--prune=0`)
    /// are always present when using [`UtreexoDConf::default`].
    pub args: Vec<&'a str>,

    /// Root directory under which a fresh temporary working directory is
    /// created for each instance. Falls back to the `TEMPDIR_ROOT`
    /// environment variable, then the system temp dir.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory. The directory is created if it does not
    /// exist. Data survives [`Drop`]; the process is stopped but files are
    /// kept so you can inspect or reuse them.
    pub staticdir: Option<PathBuf>,

    /// How many times to retry spawning `utreexod` before giving up.
    ///
    /// Each attempt picks fresh random ports, so transient port-collision
    /// errors are automatically recovered from. Defaults to [`NODE_BUILDING_ATTEMPTS`].
    pub max_retries: u8,
}

impl Default for UtreexoDConf<'_> {
    fn default() -> Self {
        UtreexoDConf {
            args: vec![
                "--regtest",
                "--notls",
                "--nodnsseed",
                "--cfilters",
                "--prune=0",
                "--noassumeutreexo",
                "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
            ],
            tmpdir: None,
            staticdir: None,
            max_retries: NODE_BUILDING_ATTEMPTS,
        }
    }
}

/// A running `utreexod` regtest node.
///
/// The node is started in [`UtreexoD::from_bin`] (or one of its siblings) and
/// stopped — and its temporary files removed — when this value is dropped.
///
/// # Authentication
///
/// Unlike `bitcoind`, `utreexod` does not use cookie files. RPC authentication
/// uses a hardcoded username/password pair (`halfin`/`halfin`) set at startup.
///
/// # Networking
///
/// Both the RPC and P2P ports are chosen from the OS's ephemeral range at
/// startup. Use [`UtreexoD::get_rpc_socket`] and [`UtreexoD::get_p2p_socket`]
/// to discover them after construction.
#[derive(Debug)]
pub struct UtreexoD {
    /// Handle to the spawned `utreexod` child process.
    process: Child,

    /// Authenticated JSON-RPC client connected to the node.
    pub client: Client,

    /// Owns (and optionally cleans up) the node's data directory.
    working_directory: DataDir,

    /// Address the JSON-RPC server is bound to.
    rpc_socket: SocketAddr,

    /// Address the P2P listener is bound to.
    p2p_socket: SocketAddr,
}

#[rustfmt::skip]
impl Node for UtreexoD {
    fn get_name() -> &'static str { "UtreexoD" }

    fn get_bin_name() -> &'static str { "utreexod_v_0_6_0" }

    fn get_p2p_socket(&self) -> SocketAddr { self.get_p2p_socket() }

    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> { self.has_peer(socket) }

    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> { self.add_peer(socket) }

    fn get_peer_count(&self) -> Result<u32, Error> { self.get_peer_count() }

    fn get_chain_tip(&self) -> Result<u32, Error> {
        let height = self.get_chain_tip()?;
        if height == 0 {
            return Err(
                Error::UnexpectedResponse("utreexod is at genesis, the proof index not ready yet".to_string())
            );
        }
        self.get_block_uproof(height)?;
        Ok(height)
    }

    fn get_filter_tip(&self) -> Result<u32, Error> { self.get_filter_tip() }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> { self.get_block_hash(height) }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        self.client.call(method, args).map_err(Error::JsonRpc)
    }

    fn poll_interval() -> Duration { 2 * POLL_INTERVAL }

    fn wait_timeout() -> Duration { 2 * WAIT_TIMEOUT }
}

impl UtreexoD {
    // ----> NODE

    /// Start a [`UtreexoD`] node using the binary located by [`get_utreexod_path`], with the default [`UtreexoDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `github.com` per `build.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary cannot be located or the node cannot be started.
    pub fn new() -> Result<Self, Error> {
        Self::from_bin(get_utreexod_path()?)
    }

    /// Start a [`UtreexoD`] node using the binary located by [`get_utreexod_path`], with a custom [`UtreexoDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `github.com` per `build.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary cannot be located, the configuration is invalid, or the node cannot be started.
    pub fn new_with_conf(conf: &UtreexoDConf) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_utreexod_path()?, conf)
    }

    /// Create a [`UtreexoD`] instance running the binary at [`Path`] with the default [`UtreexoDConf`].
    ///
    /// # Errors
    ///
    /// Returns an error if `utreexod_bin` is invalid or the node cannot be started.
    pub fn from_bin<P: AsRef<Path>>(utreexod_bin: P) -> Result<Self, Error> {
        Self::from_bin_with_conf(utreexod_bin, &UtreexoDConf::default())
    }

    /// Create a [`UtreexoD`] instance running the binary at [`Path`] with a custom [`UtreexoDConf`].
    ///
    /// The method retries up to [`UtreexoDConf::max_retries`] times. On each
    /// attempt it:
    ///
    /// 1. Picks fresh ephemeral RPC and P2P ports.
    /// 2. Spawns `utreexod` with those ports and a fresh data directory.
    /// 3. Waits for the RPC server to become responsive (up to 10 s).
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is invalid, the working directory
    /// cannot be created, RPC setup fails, or all attempts are exhausted.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        utreexod_bin: P,
        conf: &UtreexoDConf,
    ) -> Result<Self, Error> {
        // Validate the `bitcoind_bin` path
        let utreexod_bin = utreexod_bin.as_ref();
        // The path must be absolute
        if !utreexod_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: utreexod_bin.display().to_string(),
            });
        }
        // The path must be a file
        if !utreexod_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: utreexod_bin.display().to_string(),
            });
        }

        for _attempt in 0..conf.max_retries {
            let working_directory = Self::init_work_dir(conf)?;

            #[cfg(target_os = "windows")]
            Self::prepare_sparse_forest_file(&working_directory)?;

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, rpc_port));
            let rpc_url = format!("http://{}", rpc_socket);

            let p2p_port = get_available_port();
            let p2p_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, p2p_port));

            let datadir_arg = format!("--datadir={}", working_directory.path().display());
            let rpclisten_arg = format!("--rpclisten=127.0.0.1:{}", rpc_port);
            let rpcuser_arg = format!("--rpcuser={}", RPC_USER);
            let rpcpass_arg = format!("--rpcpass={}", RPC_PASS);
            let listen_arg = format!("--listen=127.0.0.1:{}", p2p_port);
            let proof_index_max_memory_arg = "--utreexoproofindexmaxmemory=256".to_string();

            debug!(
                "Spawning {} [RPC_SOCKET={}, P2P_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                rpc_socket,
                p2p_socket,
                working_directory.path().display()
            );

            let mut process = Command::new(utreexod_bin)
                .args(&conf.args)
                .arg(&datadir_arg)
                .arg(&rpclisten_arg)
                .arg(&rpcuser_arg)
                .arg(&rpcpass_arg)
                .arg(&listen_arg)
                .arg("--prune=0")
                .arg("--flatutreexoproofindex")
                .arg(&proof_index_max_memory_arg)
                .arg("--v2transport")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            // Add a small timeout to let `bitcoind` fail
            // and retry in the case of a port collision.
            thread::sleep(NODE_BUILDING_INTERVAL);

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(status)) => {
                    let output = process.wait_with_output().map_err(Error::Io)?;
                    eprintln!(
                        "{} exited immediately with status={status}; stdout={}; stderr={}",
                        Self::get_name(),
                        String::from_utf8_lossy(&output.stdout).trim(),
                        String::from_utf8_lossy(&output.stderr).trim(),
                    );
                    continue;
                }
                Err(err) => {
                    debug!(
                        "{} status check failed, retrying with fresh ports: {}",
                        Self::get_name(),
                        err
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
                pipe_to_tracing(stdout, "utreexod");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "utreexod");
            }

            let auth = Auth::UserPass(RPC_USER.to_string(), RPC_PASS.to_string());
            if let Ok(client) = Self::wait_for_client(&rpc_url, &auth, Duration::from_secs(10)) {
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
                    rpc_socket,
                    p2p_socket,
                });
            }
            let _ = process.kill();
        }

        Err(Error::ExhaustedNodeBuildingAttempts(conf.max_retries))
    }

    /// Send `stop` via RPC and wait for the process to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically. It is provided for cases where you
    /// need the exit status or want to ensure the node has fully shut down
    /// before proceeding.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC stop call fails or the child process cannot be waited on.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());

        // Send a `stop` over RPC.
        let _ = self.client.stop().map_err(Error::FailedToStop)?;
        // Wait for the process to terminate and get its exit status.
        let exit_status = self.process.wait().map_err(Error::Io)?;

        Ok(exit_status)
    }

    /// Return the OS process ID of the running `utreexod` process.
    pub fn get_pid(&self) -> u32 {
        let pid = self.process.id();

        debug!("{}: got pid={}", Self::get_name(), pid);

        pid
    }

    /// Get [`UtreexoD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        let working_directory = self.working_directory.path();

        debug!(
            "{}: got working directory at path={}",
            Self::get_name(),
            working_directory.display()
        );

        working_directory
    }

    /// Get a reference to [`UtreexoD`]'s RPC [`Client`].
    pub fn get_rpc_client(&self) -> &Client {
        debug!(
            "{}: got rpc client for socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        &self.client
    }

    /// Return the JSON-RPC [`SocketAddr`] the node is listening on.
    pub fn get_rpc_socket(&self) -> SocketAddr {
        debug!(
            "{}: got rpc socket at socket={}",
            Self::get_name(),
            self.rpc_socket
        );

        self.rpc_socket
    }

    /// Return the P2P [`SocketAddr`] the node is listening on.
    ///
    /// Pass this to [`UtreexoD::add_peer`] on another node to connect the two.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        debug!(
            "{}: got p2p socket at socket={}",
            Self::get_name(),
            self.p2p_socket
        );

        self.p2p_socket
    }

    // ----> RPC CALL WRAPPERS

    /// Get the current chain height.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails or the response does not contain a block height.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let height = self
            .client
            .call::<serde_json::Value>("getblockchaininfo", &[])
            .map_err(Error::JsonRpc)?["blocks"]
            .as_u64()
            .ok_or(Error::UnexpectedResponse(
                "getblockchaininfo returned no `blocks` field".to_string(),
            ))? as u32;

        debug!("{}: got chain tip at height={}", Self::get_name(), height);

        Ok(height)
    }

    /// Get the current filter height.
    ///
    /// # Errors
    ///
    /// Returns an error if the chain tip, block hash, or compact-filter RPC call fails.
    pub fn get_filter_tip(&self) -> Result<u32, Error> {
        let height = self.get_chain_tip()?;
        let hash = self.get_block_hash(height)?;

        self.client
            .call::<serde_json::Value>(
                "getcfilterheader",
                &[
                    serde_json::Value::String(hash.to_string()),
                    serde_json::Value::Number(0.into()),
                ],
            )
            .map_err(Error::JsonRpc)?;

        debug!("{}: got filter tip at height={}", Self::get_name(), height);

        Ok(height)
    }

    /// Get the [`BlockHash`] of the block at height `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails or the response is not a valid block hash.
    pub fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        let hash = self
            .client
            .call::<serde_json::Value>("getblockhash", &[height.into()])
            .map_err(Error::JsonRpc)?
            .as_str()
            .ok_or(Error::UnexpectedResponse(
                "getblockhash returned a non-string value".to_string(),
            ))?
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

    // TODO(@luisschwab): return a `rustreexo::proof::Proof`
    /// Get the Utreexo proof for the block at height `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the block hash lookup fails, the proof RPC call fails,
    /// or the response is not a string.
    pub fn get_block_uproof(&self, height: u32) -> Result<String, Error> {
        debug!(
            "{}: fetching utreexo proof for height={}",
            Self::get_name(),
            height
        );

        let block_hash = self.get_block_hash(height)?;
        let proof_hex = self
            .client
            .call::<serde_json::Value>("getutreexoproof", &[block_hash.to_string().into()])
            .map_err(Error::JsonRpc)?
            .as_str()
            .ok_or(Error::UnexpectedResponse(
                "getutreexoproof returned a non-string value".to_string(),
            ))?
            .to_string();
        Ok(proof_hex)
    }

    /// Check whether this [`UtreexoD`] has a peer with a specific [`SocketAddr`].
    ///
    /// # Errors
    ///
    /// Returns an error if the peer-info JSON-RPC call fails.
    pub fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> {
        let peers = self
            .client
            .call::<serde_json::Value>("getpeerinfo", &[])
            .map_err(Error::JsonRpc)?;

        let has_peer = peers.as_array().is_some_and(|v| {
            v.iter().any(|p| {
                let inbound = p["inbound"].as_bool().unwrap_or(false);
                if inbound {
                    // For inbound connections, `addr` is the peer's ephemeral port
                    // and `addrlocal` is our own listening port — neither gives us
                    // the peer's listening port, so we can't match on socket directly.
                    // Instead, match on `addrlocal` == our own socket as a proxy
                    // for confirming the connection is established.
                    p["addrlocal"]
                        .as_str()
                        .is_some_and(|a| a.contains(&self.p2p_socket.to_string()))
                } else {
                    // For outbound connections, `addr` is the peer's listening port.
                    p["addr"]
                        .as_str()
                        .is_some_and(|a| a.contains(&socket.to_string()))
                }
            })
        });

        debug!(
            "{}: checked peer connection at socket={} connected={}",
            Self::get_name(),
            socket,
            has_peer
        );

        Ok(has_peer)
    }

    /// Connect this [`UtreexoD`] to a peer at [`socket`](SocketAddr) and
    /// wait until the connection is established (up to 5 seconds with exponential back-off).
    ///
    /// # Errors
    ///
    /// Returns an error if the add-node RPC call fails or the peer does not
    /// appear in `getpeerinfo` within the timeout.
    pub fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> {
        debug!("{}: adding peer with socket={}", Self::get_name(), socket);

        self.client
            .add_node(&socket.to_string(), AddNodeCommand::Add)
            .map_err(Error::JsonRpc)?;

        let mut delay = CONNECTION_INTERVAL;

        let start = Instant::now();
        while start.elapsed() < CONNECTION_TIMEOUT {
            let peers = self
                .client
                .call::<serde_json::Value>("getpeerinfo", &[])
                .map_err(Error::JsonRpc)?;
            if peers.as_array().is_some_and(|v| {
                v.iter().any(|p| {
                    p["addr"]
                        .as_str()
                        .is_some_and(|a| a.contains(&socket.to_string()))
                })
            }) {
                debug!("{}: connected peer at socket={}", Self::get_name(), socket);
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

    /// Get [`UtreexoD`]'s peer count.
    ///
    /// # Errors
    ///
    /// Returns an error if the peer-info JSON-RPC call fails or returns a non-array response.
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let peers = self
            .client
            .call::<serde_json::Value>("getpeerinfo", &[])
            .map_err(Error::JsonRpc)?;
        let peer_count = peers
            .as_array()
            .ok_or(Error::UnexpectedResponse(
                "getpeerinfo returned a non-array value".to_string(),
            ))?
            .len() as u32;

        debug!("{}: got peer count value={}", Self::get_name(), peer_count);

        Ok(peer_count)
    }

    /// Generate `count` blocks.
    ///
    /// Returns the block hashes as a [`Vec<BlockHash>`].
    ///
    /// # Errors
    ///
    /// Returns an error if block generation fails or the response contains an invalid block hash.
    pub fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> {
        debug!("{}: generating count={} block(s)", Self::get_name(), count);

        let hashes = self
            .client
            .call::<serde_json::Value>("generate", &[serde_json::Value::Number(count.into())])
            .map_err(Error::JsonRpc)?
            .as_array()
            .ok_or(Error::UnexpectedResponse(
                "generate returned a non-array value".to_string(),
            ))?
            .iter()
            .map(|h| {
                h.as_str()
                    .ok_or(Error::UnexpectedResponse(
                        "generate returned a non-string hash".to_string(),
                    ))?
                    .parse::<BlockHash>()
                    .map_err(|e| Error::UnexpectedResponse(e.to_string()))
            })
            .collect::<Result<Vec<BlockHash>, Error>>()?;
        Ok(hashes)
    }

    // ----> INTERNAL

    /// Resolve and create the working directory according to `conf`.
    ///
    /// Precedence: `conf.tmpdir` → `TEMPDIR_ROOT` env var → system temp.
    /// If `conf.staticdir` is set the directory is created but never cleaned
    /// up automatically.
    fn init_work_dir(conf: &UtreexoDConf) -> Result<DataDir, Error> {
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
                    .prefix("halfin-utreexod-")
                    .tempdir_in(tmpdir)
                    .map_err(Error::Io)?,
            ),
            (None, None) => DataDir::Temporary(
                tempfile::Builder::new()
                    .prefix("halfin-utreexod-")
                    .tempdir()
                    .map_err(Error::Io)?,
            ),
        };

        Ok(work_dir)
    }

    /// Mark the Utreexo forest data file as sparse before `utreexod` opens it.
    ///
    /// The `utreexo::OpenForest` call truncates `forest_data.dat` to a large apparent size.
    /// Unix filesystems usually handle that as sparse automatically, but Windows requires
    /// the sparse flag to be set first.
    #[cfg(target_os = "windows")]
    fn prepare_sparse_forest_file(working_directory: &DataDir) -> Result<(), Error> {
        let forest_dir = working_directory
            .path()
            .join("regtest")
            .join("utreexostate_flat");
        fs::create_dir_all(&forest_dir).map_err(Error::Io)?;

        let forest_data_file = forest_dir.join("forest_data.dat");
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&forest_data_file)
            .map_err(Error::Io)?;

        let status = Command::new("fsutil")
            .arg("sparse")
            .arg("setflag")
            .arg(&forest_data_file)
            .status()
            .map_err(Error::Io)?;

        if status.success() {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse(format!(
                "failed to mark {} as sparse with fsutil: {}",
                forest_data_file.display(),
                status
            )))
        }
    }

    /// Poll `getblockchaininfo` until it succeeds, building and returning the
    /// authenticated client on success.
    ///
    /// Returns `Err` if the node is not responsive within `timeout`.
    fn wait_for_client(rpc_url: &str, auth: &Auth, timeout: Duration) -> Result<Client, Error> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(client) = Client::new_with_auth(rpc_url, auth.clone()) {
                if client
                    .call::<serde_json::Value>("getblockchaininfo", &[])
                    .is_ok()
                {
                    return Ok(client);
                }
            }
            thread::sleep(Duration::from_millis(200));
        }

        Err(Error::RpcClientSetupTimeout)
    }
}

impl Drop for UtreexoD {
    /// Kills the `utreexod` process.
    ///
    /// Errors from `kill` are silently discarded so that `Drop` never panics.
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            Self::get_name(),
            self.process.id()
        );
        let _ = self.process.kill();
    }
}
