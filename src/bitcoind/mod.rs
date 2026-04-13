// SPDX-License-Identifier: MIT OR Apache-2.0

//! # BitcoinD
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
//! let node = BitcoinD::download_new().unwrap();
//!
//! // Mine some blocks
//! node.generate(10).unwrap();
//! assert_eq!(node.get_height().unwrap(), 10);
//! ```
//!
//! ## Directory Handling
//!
//! By default each [`BitcoinD`] instance uses a temporary directory that is
//! cleaned up when the instance is dropped. Pass a `staticdir` in
//! [`BitcoinDConf`] to keep data between runs.

pub extern crate corepc_client as client;

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
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use corepc_client::bitcoin::Network;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v30::AddNodeCommand;
use corepc_client::client_sync::v30::Client;
use tempfile::TempDir;

use crate::DataDir;
use crate::Error;
use crate::LOCALHOST;
use crate::get_available_port;

pub use anyhow;
pub use serde_json;
pub use tempfile;
pub use which;

/// Name of the wallet created (or loaded) inside every [`BitcoinD`] instance.
const BITCOIND_WALLET: &str = "wallet";

/// Configuration for a [`BitcoinD`] instance.
///
/// Build one explicitly, or call [`BitcoinDConf::default`] for sensible regtest
/// defaults (`-regtest -fallbackfee=0.0001`).
///
/// # Directory precedence
///
/// Exactly one of `tmpdir` / `staticdir` may be set at a time; setting both
/// returns [`HalfinError::BothDirsSpecified`].
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
    /// The defaults (`-regtest`, `-fallbackfee=0.0001`) are always present when
    /// using [`BitcoinDConf::default`].  Replace or extend this vec to
    /// customise the node (e.g. add `-txindex=1`).
    pub args: Vec<&'a str>,

    /// Root directory under which a fresh temporary working directory is
    /// created for each instance.  Falls back to the `TEMPDIR_ROOT`
    /// environment variable, then the system temp dir.
    pub tmpdir: Option<PathBuf>,

    /// Persistent data directory.  The directory is created if it does not
    /// exist.  Data survives [`Drop`]; the process is stopped but files are
    /// kept so you can inspect or reuse them.
    pub staticdir: Option<PathBuf>,

    /// How many times to retry spawning `bitcoind` before giving up.
    ///
    /// Each attempt picks fresh random ports, so transient port-collision
    /// errors are automatically recovered from.  Defaults to `5`.
    pub max_retries: u8,
}

impl Default for BitcoinDConf<'_> {
    fn default() -> Self {
        BitcoinDConf {
            args: vec!["-regtest", "-fallbackfee=0.0001"],
            tmpdir: None,
            staticdir: None,
            max_retries: 5,
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
    rpc_client: Client,
    /// Owns (and optionally cleans up) the node's data directory.
    working_directory: DataDir,
    /// Path to the cookie file used for RPC authentication.
    cookie_file: PathBuf,
    /// Address the JSON-RPC server is bound to.
    rpc_socket: SocketAddr,
    /// Address the P2P listener is bound to.
    p2p_socket: SocketAddr,
}

impl Drop for BitcoinD {
    /// Gracefully stops the node (if it was started with a persistent
    /// directory) and kills the process.
    ///
    /// Errors from `stop` and `kill` are silently discarded so that `Drop`
    /// never panics.
    fn drop(&mut self) {
        if let DataDir::Persistent(_) = self.working_directory {
            let _ = self.stop();
        }
        let _ = self.process.kill();
    }
}

impl BitcoinD {
    // ----> NODE

    /// Start a [`BitcoinD`] node using the binary located by [`get_bitcoind_path`], with the default [`BitcoinDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `bitcoincore.org` per `build.rs`.
    pub fn download_new() -> anyhow::Result<BitcoinD> {
        BitcoinD::from_bin(get_bitcoind_path()?)
    }

    /// Start a [`BitcoinD`] node using the binary located by [`get_bitcoind_path`], with a custom [`BitcoinDConf`].
    ///
    /// If the binary is not cached under `target/bin/`, it will fetch one from `bitcoincore.org` per `build.rs`.
    pub fn download_new_with_conf(conf: &BitcoinDConf) -> anyhow::Result<BitcoinD> {
        BitcoinD::from_bin_with_conf(get_bitcoind_path()?, conf)
    }

    /// Create a [`BitcoinD`] instance running the binary at [`Path`] with the default [`BitcoinDConf`].
    pub fn from_bin<P: AsRef<Path>>(bitcoind_bin: P) -> anyhow::Result<BitcoinD> {
        BitcoinD::from_bin_with_conf(bitcoind_bin, &BitcoinDConf::default())
    }

    /// Create a [`BitcoinD`] instance running the binary at [`Path`] with a custom [`BitcoinDConf`].
    /// The method retries up to [`BitcoinDConf::max_retries`] times.  On each
    /// attempt it:
    ///
    /// 1. Picks fresh ephemeral RPC and P2P ports.
    /// 2. Spawns `bitcoind` with those ports and a fresh data directory.
    /// 3. Waits for the cookie file to appear (up to 5 s).
    /// 4. Creates or loads the default wallet and builds an RPC client.
    /// 5. Waits for the node to become responsive (up to 5 s).
    ///
    /// Returns an error if all attempts are exhausted.
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        bitcoind_bin: P,
        conf: &BitcoinDConf,
    ) -> anyhow::Result<BitcoinD> {
        for _ in 0..=conf.max_retries {
            let working_directory = Self::init_work_dir(conf)?;
            let cookie_file = working_directory
                .path()
                .join(Network::Regtest.to_string())
                .join(".cookie");

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(LOCALHOST, rpc_port));
            let rpc_url = format!("http://{}", rpc_socket);

            let p2p_port = get_available_port();
            let p2p_socket = SocketAddr::V4(SocketAddrV4::new(LOCALHOST, p2p_port));

            let datadir_arg = format!("-datadir={}", working_directory.path().display());
            let rpc_arg = format!("-rpcport={}", rpc_port);
            let p2p_arg = format!("-bind={}", p2p_socket);

            let mut process = Command::new(bitcoind_bin.as_ref())
                .args(&conf.args)
                .arg(&datadir_arg)
                .arg(&rpc_arg)
                .arg(&p2p_arg)
                .stdout(Stdio::null())
                .spawn()
                .with_context(|| format!("Error while executing {:?}", bitcoind_bin.as_ref()))?;

            // If the process exited immediately, try again with new ports.
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    let _ = process.kill();
                    continue;
                }
                Ok(None) => {}
            }

            if Self::wait_for_cookie_file(&cookie_file, Duration::from_secs(5)).is_err() {
                let _ = process.kill();
                continue;
            }

            let auth = Auth::CookieFile(cookie_file.clone());
            let client_base = Self::create_base_rpc_client(&rpc_url, &auth)?;

            let wallet_url = format!("{}/wallet/{}", rpc_url, BITCOIND_WALLET);
            let deadline = Instant::now() + Duration::from_secs(10);
            let rpc_client = loop {
                if Instant::now() > deadline {
                    let _ = process.kill();
                    return Err(anyhow::anyhow!("timeout creating/loading wallet"));
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

            if Self::wait_for_client(&rpc_client, Duration::from_secs(5)).is_err() {
                let _ = process.kill();
                continue;
            }

            return Ok(BitcoinD {
                process,
                rpc_client,
                working_directory,
                cookie_file,
                rpc_socket,
                p2p_socket,
            });
        }
        Err(anyhow::anyhow!(
            "Failed to start bitcoind after {} attempts",
            conf.max_retries
        ))
    }

    /// Send `stop` via RPC and wait for the process to exit.
    ///
    /// Calling this method is **not required** in normal usage because [`Drop`]
    /// kills the process automatically.  It is provided for cases where you
    /// need the exit status or want to ensure the node has fully shut down
    /// before proceeding.
    pub fn stop(&mut self) -> anyhow::Result<ExitStatus> {
        self.rpc_client.stop()?;
        Ok(self.process.wait()?)
    }

    /// Return the OS process ID of the running `bitcoind` process.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }

    /// Get [`BitcoinD`]'s data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }

    /// Return the P2P [`SocketAddr`] the node is listening on.
    ///
    /// Pass this to [`BitcoinD::add_peer`] on another node to connect the two.
    pub fn get_p2p_socket(&self) -> SocketAddr {
        self.p2p_socket
    }

    /// Get a reference to [`BitcoinD`]'s RPC [`Client`].
    pub fn get_rpc_client(&self) -> &Client {
        &self.rpc_client
    }

    /// Return the JSON-RPC [`SocketAddr`] the node is listening on.
    pub fn rpc_socket(&self) -> SocketAddr {
        self.rpc_socket
    }

    /// Get the [`Path`] to [`BitcoinD`]'s cookie file.
    pub fn cookie_file(&self) -> &Path {
        &self.cookie_file
    }

    // ----> RPC CALL WRAPPERS

    /// Get the current chain height.
    pub fn get_height(&self) -> anyhow::Result<u32> {
        let height = self.rpc_client.get_blockchain_info()?.blocks as u32;
        Ok(height)
    }

    /// Connect this [`BitcoinD`] to another [`BitcoinD`] at `socket` and
    /// wait until the connection is established (up to 5 seconds with exponential back-off).
    ///
    /// Returns an error if the peer does not appear in `getpeerinfo` within the timeout.
    pub fn add_peer(&self, socket: SocketAddr) -> anyhow::Result<()> {
        self.rpc_client
            .add_node(&socket.to_string(), AddNodeCommand::Add)?;

        let mut delay = Duration::from_millis(100);
        let timeout = Duration::from_secs(5);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let peers = self.rpc_client.get_peer_info()?;
            if peers
                .0
                .iter()
                .any(|p| p.address.contains(&socket.to_string()))
            {
                return Ok(());
            }
            thread::sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(anyhow::anyhow!(
            "timeout waiting for peer {} to connect",
            socket
        ))
    }

    /// Get [`BitcoinD`]'s peer count.
    pub fn get_peer_count(&self) -> anyhow::Result<u32> {
        let peer_count = self.rpc_client.get_peer_info()?.0.len() as u32;
        Ok(peer_count)
    }

    /// Generate `count` blocks.
    pub fn generate(&self, count: usize) -> anyhow::Result<()> {
        let address = self.rpc_client.new_address()?;
        self.rpc_client.generate_to_address(count, &address)?;
        Ok(())
    }

    // ----> INTERNAL

    /// Resolve and create the working directory according to `conf`.
    ///
    /// Precedence: `conf.tmpdir` → `TEMPDIR_ROOT` env var → system temp.
    /// If `conf.staticdir` is set the directory is created but never cleaned
    /// up automatically.
    fn init_work_dir(conf: &BitcoinDConf) -> anyhow::Result<DataDir> {
        let tmpdir = conf
            .tmpdir
            .clone()
            .or_else(|| env::var("TEMPDIR_ROOT").map(PathBuf::from).ok());
        let work_dir = match (&tmpdir, &conf.staticdir) {
            (Some(_), Some(_)) => return Err(Error::BothDirsSpecified.into()),
            (Some(tmpdir), None) => DataDir::Temporary(TempDir::new_in(tmpdir)?),
            (None, Some(workdir)) => {
                fs::create_dir_all(workdir)?;
                DataDir::Persistent(workdir.to_owned())
            }
            (None, None) => DataDir::Temporary(TempDir::new()?),
        };
        Ok(work_dir)
    }

    /// Attempt to create a base (wallet-less) RPC client, retrying up to 10
    /// times with 200 millisecond gaps. Used during startup before the wallet exists.
    fn create_base_rpc_client(rpc_url: &str, auth: &Auth) -> anyhow::Result<Client> {
        for _ in 0..10 {
            if let Ok(client) = Client::new_with_auth(rpc_url, auth.clone()) {
                return Ok(client);
            }
            thread::sleep(Duration::from_millis(200));
        }
        Client::new_with_auth(rpc_url, auth.clone())
            .map_err(|e| Error::BitcoinDNotRunning(e.to_string()).into())
    }

    /// Poll for the cookie file's existence, sleeping 200 milliseconds between checks.
    ///
    /// Returns `Err` if the file does not appear within `timeout`.
    fn wait_for_cookie_file(cookie_file: &Path, timeout: Duration) -> anyhow::Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cookie_file.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(anyhow::anyhow!(
            "timeout waiting for cookie file: {}",
            cookie_file.display()
        ))
    }

    /// Poll `getblockchaininfo` until it succeeds, sleeping 200 milliseconds between attempts.
    ///
    /// Returns `Err` if the node is not responsive within `timeout`.
    fn wait_for_client(client: &Client, timeout: Duration) -> anyhow::Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if client
                .call::<serde_json::Value>("getblockchaininfo", &[])
                .is_ok()
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(anyhow::anyhow!("timeout waiting for bitcoind to be ready"))
    }
}

/// Return the path to the downloaded `bitcoind` binary.
pub fn get_bitcoind_path() -> anyhow::Result<PathBuf> {
    use versions::BITCOIND_VERSION;

    let mut path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bin");

    path.push(format!("bitcoin-{}", BITCOIND_VERSION));
    path.push("bitcoind");

    if !path.exists() {
        return Err(anyhow::anyhow!("bitcoind binary not found at {:?}", path));
    }
    Ok(path)
}

#[cfg(test)]
mod test {
    use super::*;

    /// Block the calling thread until `node` reaches at least `height`, or panic after 10 seconds.
    fn wait_for_height(node: &BitcoinD, height: u32) {
        let timeout = Duration::from_secs(10);
        let start = Instant::now();
        while start.elapsed() < timeout {
            if node.get_height().unwrap() >= height {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("timeout waiting for node to reach height {}", height);
    }

    /// Verify that [`BitcoinD`] starts successfully and exposes its PID, working directory, and P2P socket
    #[test]
    fn test_bitcoind_starts() {
        let bin = get_bitcoind_path().unwrap();
        let bitcoind = BitcoinD::from_bin(bin).unwrap();

        println!("PID: {}", bitcoind.get_pid());
        println!("Working Directory: {:?}", bitcoind.get_working_directory());
        println!("P2P Socket: {}", bitcoind.get_p2p_socket());
    }

    /// Verify that `generate` mines the requested number of blocks.
    #[test]
    fn test_bitcoind_generate() {
        let bitcoind = BitcoinD::download_new().unwrap();

        let height = bitcoind.get_height().unwrap();
        assert_eq!(height, 0);

        bitcoind.generate(10).unwrap();

        let height = bitcoind.get_height().unwrap();
        assert_eq!(height, 10);
    }

    /// Verify that two nodes can connect to each other via `add_peer` and
    /// that the peer count reflects the new connection on both sides.
    #[test]
    fn test_bitcoind_addnode() {
        let bitcoind_alpha = BitcoinD::download_new().unwrap();
        let bitcoind_beta = BitcoinD::download_new().unwrap();

        assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 0);
        assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 0);

        bitcoind_beta
            .add_peer(bitcoind_alpha.get_p2p_socket())
            .unwrap();

        assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 1);
        assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 1);
    }

    /// Verify that blocks mined on one node propagate to a connected peer.
    #[test]
    fn test_bitcoind_blocks_propagate() {
        let bitcoind_alpha = BitcoinD::download_new().unwrap();
        let bitcoind_beta = BitcoinD::download_new().unwrap();

        bitcoind_alpha.generate(21).unwrap();

        assert_eq!(bitcoind_alpha.get_height().unwrap(), 21);
        assert_eq!(bitcoind_beta.get_height().unwrap(), 0);

        bitcoind_alpha
            .add_peer(bitcoind_beta.get_p2p_socket())
            .unwrap();

        wait_for_height(&bitcoind_beta, 21);
        assert_eq!(bitcoind_beta.get_height().unwrap(), 21);

        bitcoind_beta.generate(21).unwrap();
        wait_for_height(&bitcoind_alpha, 42);
        assert_eq!(bitcoind_alpha.get_height().unwrap(), 42);
    }
}
