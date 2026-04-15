//! # Halfin
//!
//! A bitcoin node running utility for integration testing.
//!
//! ## Supported Implementations and Versions
//!
//! | Implementation | Version  | Feature Flag     |
//! |----------------|----------|----------------- |
//! | [`bitcoind`]   | `v30.2`  | `bitcoind_30_2`  |
//! | [`utreexod`]   | `v0.5.0` | `utreexod_0_5_0` |
//!
//! ## Example
//!
//! ```rust,no_run
//! use halfin::bitcoind::BitcoinD;
//! use halfin::utreexod::UtreexoD;
//!
//! let bitcoind = BitcoinD::download_new().unwrap();
//! bitcoind.generate(10).unwrap();
//! assert_eq!(bitcoind.get_height().unwrap(), 10);
//!
//! let utreexod = UtreexoD::download_new().unwrap();
//! utreexod.generate(10).unwrap();
//! assert_eq!(utreexod.get_height().unwrap(), 10);
//! ```
//!
//! [`bitcoind`]: <https://github.com/bitcoin/bitcoin>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>

use core::error;
use core::fmt;
use core::net::Ipv4Addr;
use core::net::SocketAddr;
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;

pub use bitcoind::BitcoinD;
pub use utreexod::UtreexoD;

pub mod bitcoind;
pub mod utreexod;

/// IPv4 Localhost address.
const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// The maximum number of attempts at instantiating a [`BitcoinD`]/[`UtreexoD`].
pub const NODE_BUILDING_MAX_RETRIES: u8 = 5;

/// The [`Duration`] between polls for `wait_for_height`.
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The timeout [`Duration`] for `wait_for_height`.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Common interface across all node implementations ([`BitcoinD`]/[`UtreexoD`]).
pub trait Node {
    /// A human-readable name for the [`Node`].
    fn get_name() -> &'static str;

    /// Get the [`Node`]'s current chain height.
    fn get_height(&self) -> Result<u32, Error>;

    /// How long to sleep between `get_height` RPC calls.
    ///
    /// Defaults to [`POLL_INTERVAL`].
    ///
    /// Override for nodes that need a longer settling time between RPC calls.
    fn poll_interval() -> Duration {
        POLL_INTERVAL
    }

    /// How long `wait_for_height` will poll before giving up.
    ///
    /// Defaults to [`WAIT_TIMEOUT`].
    ///
    /// Override for nodes that need more time to process blocks
    /// (e.g. [`UtreexoD`] needs more time to build the Merkle forest).
    fn wait_timeout() -> Duration {
        WAIT_TIMEOUT
    }
}

#[rustfmt::skip]
impl Node for BitcoinD {
    fn get_name() -> &'static str { "bitcoind" }
    fn get_height(&self) -> Result<u32, Error> { self.get_height() }
}

#[rustfmt::skip]
impl Node for UtreexoD {
    fn get_name() -> &'static str { "utreexod" }
    fn get_height(&self) -> Result<u32, Error> { self.get_height() }
    fn poll_interval() -> Duration { 2 * POLL_INTERVAL }
    fn wait_timeout() -> Duration { 2 * WAIT_TIMEOUT }
}

/// Poll a [`Node`] until its chain height reaches `height`, then return.
///
/// Panics if the node does not reach `height` within [`Node::wait_timeout`].
pub fn wait_for_height<N: Node>(node: &N, height: u32) -> Result<(), Error> {
    let start = Instant::now();
    while start.elapsed() < N::wait_timeout() {
        if node.get_height().unwrap() >= height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_height().unwrap();
    Err(Error::ChainSyncTimeOut((
        height,
        curr_height,
        N::wait_timeout(),
    )))
}

/// Poll a [`Node`] until its chain height reaches `height`, then return.
///
/// Panics if the node does not reach `height` within `timeout`.
pub fn wait_for_height_with_timeout<N: Node>(
    node: &N,
    height: u32,
    timeout: Duration,
) -> Result<(), Error> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if node.get_height().unwrap() >= height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_height().unwrap();
    Err(Error::ChainSyncTimeOut((height, curr_height, timeout)))
}

/// Ask the OS for an available port, immediately unbind and return it.
///
/// Inlining is needed to curb TOCTOU race conditions.
#[inline]
pub fn get_available_port() -> u16 {
    TcpListener::bind((LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Owns a node's working directory, either as a temporary or a persistent path.
///
/// * [`DataDir::Temporary`]: backed by a [`TempDir`]; the directory is
///   deleted automatically when this value is dropped.
/// * [`DataDir::Persistent`]: backed by a plain [`PathBuf`]; the directory
///   survives the process and is never cleaned up automatically.
#[derive(Debug)]
pub enum DataDir {
    /// A persistent directory that is **not** cleaned up on drop.
    Persistent(PathBuf),
    /// A temporary directory that is deleted when this value is dropped.
    Temporary(TempDir),
}

impl DataDir {
    /// Return the underlying filesystem path regardless of variant.
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Persistent(path) => path.to_owned(),
            Self::Temporary(tmp_dir) => tmp_dir.path().to_path_buf(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// The binary was not found at the expected location.
    BinaryNotFound(PathBuf),
    /// Failed to spawn a [process](std::process::Child) for [`BitcoinD`]/[`UtreexoD`].
    FailedToSpawn(std::io::Error),
    /// Failed to instantiate a [`BitcoinD`]/[`UtreexoD`] after [`NODE_BUILDING_MAX_RETRIES`] attempts.
    ExhaustedNodeBuildingRetries,
    /// Failed to stop [`BitcoinD`] or [`UtreexoD`] over JSON-RPC (e.g. `bitcoin-cli -regtest stop`).
    FailedToStop(corepc_client::client_sync::Error),
    /// I/O errors.
    Io(std::io::Error),
    /// JSON-RPC Errors.
    JsonRpc(corepc_client::client_sync::Error),
    /// Timed out whilst waiting for peer connection to succeed.
    PeerConnectionTimeout((SocketAddr, SocketAddr)),
    /// Both `tmpdir` and `workdir` were specified.
    BothDirsSpecified,
    /// [`BitcoinD`] is unresponsive (it's probably not running).
    UnresponsiveBitcoinD(corepc_client::client_sync::Error),
    /// [`UtreexoD`] is unresponsive (it's probably not running).
    UnresponsiveUtreexoD(corepc_client::client_sync::Error),
    /// Timed out whilst waiting for the cookie file to be generated.
    CookieFileTimeout(PathBuf),
    /// Timed out whilst waiting for the JSON-RPC client to be ready.
    RpcClientSetupTimeout,
    /// Received an unexpected response from the JSON-RPC server
    UnexpectedResponse,
    /// Timed out whilst waiting for the [`Node`]'s chain to synchronize up to `height`
    ChainSyncTimeOut((u32, u32, Duration)), // (current_height, target_height, timeout)
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            BinaryNotFound(path) => write!(f, "The `utreexod` binary was not found at the expected location: {}", path.display()),
            FailedToSpawn(err) => write!(f, "Failed to spawn a process for the node: {err:?}"),
            ExhaustedNodeBuildingRetries => write!(f, "Failed to instantiate the node after {} attempts", NODE_BUILDING_MAX_RETRIES),
            FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err:?}"),
            Io(err) => write!(f, "I/O Error: {err:?}"),
            JsonRpc(err) => write!(f, "JSON-RPC Error: {err:?}"),
            PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            BothDirsSpecified => write!(f, "Both `tempdir` and `workdir` were specified. You must choose one and only one"),
            UnresponsiveBitcoinD(err) => write!(f, "`BitcoinD` is unresponsive to JSON-RPC calls: {err:?}"),
            UnresponsiveUtreexoD(err) => write!(f, "`UtreexoD` is unresponsive to JSON-RPC calls: {err:?}"),
            CookieFileTimeout(cookie_path) => write!(f, "Timed out whilst waiting for the cookie={} to be generated", cookie_path.display()),
            RpcClientSetupTimeout => write!(f, "Timed out whilst waiting for the JSON-RPC client to be ready"),
            UnexpectedResponse => write!(f, "Received an unexpected response from the JSON-RPC server"),
            ChainSyncTimeOut((target_height, current_height, t)) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the node's chain to synchronize to height={} (current height={})",
                target_height, current_height, t.as_secs()
            ),
        }
    }
}

impl error::Error for Error {}
