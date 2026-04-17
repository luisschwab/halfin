// SPDX-License-Identifier: MIT OR Apache-2.0

//! # UtreexoD
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
use std::time::Duration;
use std::time::Instant;

use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v17::AddNodeCommand;
use corepc_client::client_sync::v17::Client;
use tempfile::TempDir;

use crate::DataDir;
use crate::Error;
use crate::LOCALHOST;
use crate::NODE_BUILDING_MAX_RETRIES;
use crate::Node;
use crate::get_available_port;

mod versions;

/// Username for RPC authentication.
const RPC_USER: &str = "halfin";

/// Password for RPC authentication.
const RPC_PASS: &str = "halfin";

/// Configuration for a [`UtreexoD`] instance.
///
/// Build one explicitly or call [`UtreexoDConf::default`] for sensible regtest
/// defaults (`--regtest --notls --nodnsseed --noassumeutreexo`).
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
    /// The defaults (`--regtest`, `--notls`, `--nodnsseed`, `--noassumeutreexo`)
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
    /// errors are automatically recovered from. Defaults to [`NODE_BUILDING_MAX_RETRIES`].
    pub max_retries: u8,
}

impl Default for UtreexoDConf<'_> {
    fn default() -> Self {
        UtreexoDConf {
            args: vec![
                "--regtest",
                "--notls",
                "--nodnsseed",
                "--noassumeutreexo",
                "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
            ],
            tmpdir: None,
            staticdir: None,
            max_retries: NODE_BUILDING_MAX_RETRIES,
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
/// startup. Use [`rpc_socket`](UtreexoD::rpc_socket) and
/// [`get_p2p_socket`](UtreexoD::get_p2p_socket) to discover them after
/// construction.
#[derive(Debug)]
pub struct UtreexoD {
    /// Handle to the spawned `utreexod` child process.
    process: Child,
    /// Authenticated JSON-RPC client connected to the node.
    rpc_client: Client,
    /// Owns (and optionally cleans up) the node's data directory.
    working_directory: DataDir,
    /// Address the JSON-RPC server is bound to.
    rpc_socket: SocketAddr,
    /// Address the P2P listener is bound to.
    p2p_socket: SocketAddr,
}

impl Drop for UtreexoD {
    /// Kills the `utreexod` process.
    ///
    /// Errors from `kill` are silently discarded so that `Drop` never panics.
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}

impl UtreexoD {
    // ----> NODE

    /// Start a [`UtreexoD`] node using the binary located by [`get_utreexod_path`], with the default [`UtreexoDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `github.com` per `build.rs`.
    pub fn new() -> Result<UtreexoD, Error> {
        UtreexoD::from_bin(get_utreexod_path()?)
    }

    /// Start a [`UtreexoD`] node using the binary located by [`get_utreexod_path`], with a custom [`UtreexoDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `github.com` per `build.rs`.
    pub fn new_with_conf(conf: &UtreexoDConf) -> Result<UtreexoD, Error> {
        UtreexoD::from_bin_with_conf(get_utreexod_path()?, conf)
    }

    /// Create a [`UtreexoD`] instance running the binary at [`Path`] with the default [`UtreexoDConf`].
    pub fn from_bin<P: AsRef<Path>>(utreexod_bin: P) -> Result<UtreexoD, Error> {
        UtreexoD::from_bin_with_conf(utreexod_bin, &UtreexoDConf::default())
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
    /// Returns an error if all attempts are exhausted.
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        utreexod_bin: P,
        conf: &UtreexoDConf,
    ) -> Result<UtreexoD, Error> {
        for _attempt in 0..conf.max_retries {
            let working_directory = Self::init_work_dir(conf)?;

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(LOCALHOST, rpc_port));
            let rpc_url = format!("http://{}", rpc_socket);

            let p2p_port = get_available_port();
            let p2p_socket = SocketAddr::V4(SocketAddrV4::new(LOCALHOST, p2p_port));

            let datadir_arg = format!("--datadir={}", working_directory.path().display());
            let rpclisten_arg = format!("--rpclisten=127.0.0.1:{}", rpc_port);
            let rpcuser_arg = format!("--rpcuser={}", RPC_USER);
            let rpcpass_arg = format!("--rpcpass={}", RPC_PASS);
            let listen_arg = format!("--listen=127.0.0.1:{}", p2p_port);

            let mut process = Command::new(utreexod_bin.as_ref())
                .args(&conf.args)
                .arg(&datadir_arg)
                .arg(&rpclisten_arg)
                .arg(&rpcuser_arg)
                .arg(&rpcpass_arg)
                .arg(&listen_arg)
                .arg("--flatutreexoproofindex")
                .arg("--utreexoproofindexmaxmemory=512")
                .arg("--v2transport")
                .stdout(Stdio::null())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            // Add a small timeout to let `bitcoind` fail
            // and retry in the case of a port collision.
            thread::sleep(Duration::from_millis(100));

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    let _ = process.kill();
                    continue;
                }
                Ok(None) => {}
            }

            let auth = Auth::UserPass(RPC_USER.to_string(), RPC_PASS.to_string());
            match Self::wait_for_client(&rpc_url, &auth, Duration::from_secs(10)) {
                Ok(rpc_client) => {
                    return Ok(UtreexoD {
                        process,
                        rpc_client,
                        working_directory,
                        rpc_socket,
                        p2p_socket,
                    });
                }
                Err(_) => {
                    let _ = process.kill();
                    continue;
                }
            }
        }

        Err(Error::ExhaustedNodeBuildingRetries)
    }

    /// Send `stop` via RPC and wait for the process to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically. It is provided for cases where you
    /// need the exit status or want to ensure the node has fully shut down
    /// before proceeding.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        // Send a `stop` over RPC.
        let _ = self.rpc_client.stop().map_err(Error::FailedToStop)?;
        // Wait for the process to terminate and get its exit status.
        let exit_status = self.process.wait().map_err(Error::Io)?;

        Ok(exit_status)
    }

    /// Return the OS process ID of the running `utreexod` process.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }

    /// Get [`UtreexoD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }

    /// Get a reference to [`UtreexoD`]'s RPC [`Client`].
    pub fn get_rpc_client(&self) -> &Client {
        &self.rpc_client
    }

    /// Return the P2P [`SocketAddr`] the node is listening on.
    ///
    /// Pass this to [`UtreexoD::add_peer`] on another node to connect the two.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        self.p2p_socket
    }

    /// Return the JSON-RPC [`SocketAddr`] the node is listening on.
    pub fn rpc_socket(&self) -> SocketAddr {
        self.rpc_socket
    }

    // ----> RPC CALL WRAPPERS

    /// Get the current chain height.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let height = self
            .rpc_client
            .call::<serde_json::Value>("getblockchaininfo", &[])
            .map_err(Error::JsonRpc)?["blocks"]
            .as_u64()
            .ok_or(Error::UnexpectedResponse)? as u32;
        Ok(height)
    }

    /// Connect this [`UtreexoD`] to a peer at `socket` and wait until the
    /// connection is established (up to 5 seconds with exponential back-off).
    ///
    /// Returns an error if the peer does not appear in `getpeerinfo` within
    /// the timeout.
    pub fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> {
        self.rpc_client
            .add_node(&socket.to_string(), AddNodeCommand::Add)
            .map_err(Error::JsonRpc)?;

        let mut delay = Duration::from_millis(100);
        let timeout = Duration::from_secs(5);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let peers = self
                .rpc_client
                .call::<serde_json::Value>("getpeerinfo", &[])
                .map_err(Error::JsonRpc)?;
            if peers
                .as_array()
                .map(|v| {
                    v.iter().any(|p| {
                        p["addr"]
                            .as_str()
                            .map(|a| a.contains(&socket.to_string()))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
            {
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
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let peers = self
            .rpc_client
            .call::<serde_json::Value>("getpeerinfo", &[])
            .map_err(Error::JsonRpc)?;
        let peer_count = peers.as_array().ok_or(Error::UnexpectedResponse)?.len() as u32;

        Ok(peer_count)
    }

    /// Generate `count` blocks.
    pub fn generate(&self, count: u32) -> Result<(), Error> {
        self.rpc_client
            .call::<serde_json::Value>("generate", &[serde_json::to_value(count).unwrap()])
            .map_err(Error::JsonRpc)?;
        Ok(())
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
            (Some(tmpdir), None) => DataDir::Temporary(TempDir::new_in(tmpdir).map_err(Error::Io)?),
            (None, None) => DataDir::Temporary(TempDir::new().map_err(Error::Io)?),
        };
        Ok(work_dir)
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

/// Return the path to the downloaded `utreexod` binary.
///
/// The path is resolved at compile time from the `HALFIN_UTREEXOD_PATH`
/// environment variable, which is set by `build.rs` after downloading
/// and extracting the binary.
pub fn get_utreexod_path() -> Result<PathBuf, Error> {
    let bin_name = UtreexoD::get_name().to_string();
    let bin_path = PathBuf::from(option_env!("HALFIN_UTREEXOD_PATH").unwrap_or(""));
    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

#[cfg(test)]
mod test {
    use crate::wait_for_height;

    use super::*;

    /// Verify that [`UtreexoD`] starts successfully and exposes its PID, working directory, and P2P socket.
    #[test]
    fn test_utreexod_starts() {
        let bin_path = get_utreexod_path().unwrap();
        let utreexod = UtreexoD::from_bin(bin_path).unwrap();

        println!("PID: {}", utreexod.get_pid());
        println!("Working Directory: {:?}", utreexod.get_working_directory());
        println!("P2P Socket: {}", utreexod.get_p2p_socket());
    }

    /// Verify that `generate` mines the requested number of blocks.
    #[test]
    fn test_utreexod_generate() {
        let utreexod = UtreexoD::new().unwrap();

        let height = utreexod.get_height().unwrap();
        assert_eq!(height, 0);

        utreexod.generate(10).unwrap();

        let height = utreexod.get_height().unwrap();
        assert_eq!(height, 10);
    }

    /// Verify that two nodes can connect to each other via `add_peer`,
    /// and that the peer count reflects the new connection on both sides.
    #[test]
    fn test_utreexod_addnode() {
        let utreexod_alpha = UtreexoD::new().unwrap();
        let utreexod_beta = UtreexoD::new().unwrap();

        assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 0);
        assert_eq!(utreexod_beta.get_peer_count().unwrap(), 0);

        utreexod_beta
            .add_peer(utreexod_alpha.get_p2p_socket())
            .unwrap();

        assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 1);
        assert_eq!(utreexod_beta.get_peer_count().unwrap(), 1);
    }

    /// Verify that mined blocks propagate to a connected peer.
    #[test]
    fn test_utreexod_blocks_propagate() {
        let utreexod_alpha = UtreexoD::new().unwrap();
        let utreexod_beta = UtreexoD::new().unwrap();

        utreexod_alpha.generate(21).unwrap();

        assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 21);
        assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 0);

        utreexod_alpha
            .add_peer(utreexod_beta.get_p2p_socket())
            .unwrap();

        wait_for_height(&utreexod_beta, 21).unwrap();
        assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 21);

        utreexod_beta.generate(21).unwrap();
        wait_for_height(&utreexod_alpha, 42).unwrap();
        assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 42);
    }
}
