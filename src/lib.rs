//! # Halfin
//!
//! A bitcoin node running utility for integration testing.
//!
//! > A {regtest} bitcoin node runner 🏃‍♂️
//!
//! This crate makes it simple to run regtest [`bitcoind`](https://github.com/bitcoin/bitcoin)
//! and [`utreexod`](https://github.com/utreexo/utreexod) instances from Rust code, useful in
//! integration test contexts.
//!
//! Pretty much [`bitcoind`](https://crates.io/crates/bitcoind) with
//! [`utreexod`](https://github.com/utreexo/utreexod) support.
//!
//! ## Supported Implementations
//!
//! | Implementation | Version  | Feature Flag     | Default Feature |
//! |----------------|----------|----------------- | --------------- |
//! | `bitcoind`     | `v31.0`  | `bitcoind_31_0`  | Yes             |
//! |                |          |                  |                 |
//! | `utreexod`     | `v0.5.2` | `utreexod_0_5_2` | Yes             |
//!
//! ## Example
//!
//! ```rust,no_run
//! use halfin::connect;
//! use halfin::bitcoind::BitcoinD;
//! use halfin::utreexod::UtreexoD;
//!
//! let bitcoind = BitcoinD::new().unwrap();
//! bitcoind.generate(10).unwrap();
//! assert_eq!(bitcoind.get_chain_tip().unwrap(), 10);
//!
//! let utreexod = UtreexoD::new().unwrap();
//! utreexod.generate(10).unwrap();
//! assert_eq!(utreexod.get_chain_tip().unwrap(), 10);
//!
//! connect(&bitcoind, &utreexod).unwrap();
//! ```
//!
//! [`bitcoind`]: <https://github.com/bitcoin/bitcoin>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>

use core::error;
use core::fmt;
use core::net::Ipv4Addr;
use core::net::SocketAddr;
use corepc_client::bitcoin::BlockHash;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;
use tracing::debug;
use tracing::trace;

pub use serde_json;

#[allow(unused)]
pub(crate) use bitcoind::BitcoinD;
#[allow(unused)]
pub(crate) use utreexod::UtreexoD;

pub mod bitcoind;
pub mod utreexod;

/// The IPv4 localhost address.
const IPV4_LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// The maximum number of attempts at instantiating a [`BitcoinD`]/[`UtreexoD`].
pub const NODE_BUILDING_MAX_RETRIES: u8 = 5;

/// The [`Duration`] between attempts at instantiating a [`Node`].
pub const NODE_BUILDING_INTERVAL: Duration = Duration::from_millis(500);

/// The [`Duration`] interval between polls for [`connect`] and [`wait_for_height`].
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The timeout [`Duration`] for [`connect`] and [`wait_for_height`].
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The interval [`Duration`] between successive attempts of node connection.
pub const CONNECTION_INTERVAL: Duration = Duration::from_millis(150);

/// The timeout [`Duration`] for node connection.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Common interface across all node implementations ([`BitcoinD`]/[`UtreexoD`]).
pub trait Node {
    /// The [`Node`]'s human-readable name.
    fn get_name() -> &'static str;

    /// The [`Node`]'s binary name.
    fn get_bin_name() -> &'static str;

    /// Get the [`Node`]'s current chain height.
    fn get_chain_tip(&self) -> Result<u32, Error>;

    /// Get the [`Node`]'s current CBF height.
    fn get_filter_tip(&self) -> Result<u32, Error>;

    // Get the [`BlockHash`] of the block at `height`.
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error>;

    /// Call a JSON-RPC `method` with the given `args` list.
    ///
    /// Response deserialization is not implemented for this method.
    ///
    /// It's up to the caller to parse the returned
    /// [`Value`](serde_json::Value) into a meaningful type.
    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error>;

    /// Get the [`Node`]'s P2P [`SocketAddr`].
    fn get_p2p_socket(&self) -> SocketAddr;

    /// Check whether the [`Node`] is connected to a peer with a specific [`SocketAddr`].
    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error>;

    /// Connect this [`Node`] to a peer at `socket` over P2P.
    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error>;

    /// Get this [`Node`]' s peer count.
    fn get_peer_count(&self) -> Result<u32, Error>;

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

/// Connect [`Node`] A to [`Node`] B.
pub fn connect<A: Node, B: Node>(a: &A, b: &B) -> Result<(), Error> {
    let socket_a = a.get_p2p_socket();
    let socket_b = b.get_p2p_socket();

    debug!("Connecting socket={} to socket={}", socket_a, socket_b);

    a.add_peer(socket_b)?;

    let is_connected =
        || -> Result<bool, Error> { Ok(a.has_peer(socket_b)? || b.has_peer(socket_a)?) };

    // Wait for either side to confirm the connection by listening port.
    // We check both because `utreexod` does not expose the peer's listening
    // port in `getpeerinfo` for inbound connections, so only one side may
    // be able to verify by socket address.
    let start = Instant::now();
    while start.elapsed() < CONNECTION_TIMEOUT {
        if is_connected()? {
            // Allow time for v2 transport negotiation to settle,
            // or for v1 fallback to complete if v2 fails, then re-verify.
            thread::sleep(CONNECTION_INTERVAL * 4);
            if is_connected()? {
                debug!("Connected socket={} to socket={}", socket_a, socket_b);
                return Ok(());
            }
        }
        thread::sleep(CONNECTION_INTERVAL);
    }

    Err(Error::ConnectionTimeout(CONNECTION_TIMEOUT))
}

/// Connect [`Node`] A to [`Node`] B and wait for them to synchronize chains.
pub fn connect_and_sync<A: Node, B: Node>(a: &A, b: &B) -> Result<(), Error> {
    connect(a, b)?;

    let height_a = a.get_chain_tip()?;
    let height_b = b.get_chain_tip()?;

    let max_height = std::cmp::max(height_a, height_b);
    wait_for_height(a, max_height)?;
    wait_for_height(b, max_height)?;

    Ok(())
}

/// Poll a [`Node`] until its chain reaches `height`.
///
/// Throws an error if the node does not reach `height` within [`Node::wait_timeout`].
pub fn wait_for_height<N: Node>(node: &N, height: u32) -> Result<(), Error> {
    debug!("Waiting for {} to reach height={}", N::get_name(), height);

    let start = Instant::now();
    while start.elapsed() < N::wait_timeout() {
        if node.get_chain_tip().unwrap() >= height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_chain_tip().unwrap();
    Err(Error::ChainSyncTimeOut((
        height,
        curr_height,
        N::wait_timeout(),
    )))
}

/// Poll a [`Node`] until its chain reaches `height` with a custom `timeout`.
///
/// Throws an error if the node does not reach `height` within `timeout`.
pub fn wait_for_height_with_timeout<N: Node>(
    node: &N,
    height: u32,
    timeout: Duration,
) -> Result<(), Error> {
    debug!(
        "Waiting for {} to reach height={} (timeout={:?})",
        N::get_name(),
        height,
        timeout
    );

    let start = Instant::now();
    while start.elapsed() < timeout {
        if node.get_chain_tip().unwrap() >= height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_chain_tip().unwrap();
    Err(Error::ChainSyncTimeOut((height, curr_height, timeout)))
}

/// Poll a [`Node`] until its Compact Block Filters reach `height`.
///
/// Throws an error if the node does not reach `filter_height` within [`Node::wait_timeout`].
pub fn wait_for_filter_height<N: Node>(node: &N, filter_height: u32) -> Result<(), Error> {
    debug!(
        "Waiting for {} to reach filter height={}",
        N::get_name(),
        filter_height
    );

    let start = Instant::now();
    while start.elapsed() < N::wait_timeout() {
        if node.get_filter_tip().unwrap() >= filter_height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_filter_height = node.get_filter_tip().unwrap();
    Err(Error::ChainSyncTimeOut((
        filter_height,
        curr_filter_height,
        N::wait_timeout(),
    )))
}

/// Spawn a background thread that reads `reader` line by line and re-emits
/// each line as a [`trace!`] event, prefixed with `source`.
///
/// Used to pipe a child [`BitcoinD`]/[`UtreexoD`] process's `stdout`/`stderr`
/// into [`tracing`]. The thread exits on EOF, which happens when the process
/// dies and its pipe is closed.
pub(crate) fn pipe_to_tracing<R: Read + Send + 'static>(reader: R, source: &'static str) {
    thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            // Skip blank lines so the trace stream mirrors the node's output.
            if !line.trim().is_empty() {
                trace!("{source}: {line}");
            }
        }
    });
}

/// Ask the OS for an available port, immediately unbind and return it.
///
/// Inlining is needed to curb TOCTOU race conditions.
#[inline]
pub fn get_available_port() -> u16 {
    TcpListener::bind((IPV4_LOCALHOST, 0))
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
    /// The binary path is not absolute.
    BinaryPathNotAbsolute { bin_name: String, path: String },

    /// The binary path is not a file.
    BinaryPathNotFile { bin_name: String, path: String },

    /// The binary was not found at the expected location.
    BinaryNotFound((String, PathBuf)),

    /// Failed to spawn a [process](std::process::Child) for [`BitcoinD`]/[`UtreexoD`].
    FailedToSpawn(std::io::Error),

    /// Failed to instantiate a [`BitcoinD`]/[`UtreexoD`] after [`NODE_BUILDING_MAX_RETRIES`] attempts.
    ExhaustedNodeBuildingRetries(u8),

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
    UnexpectedResponse(String),

    /// Timed out whilst waiting for the [`Node`]'s chain to synchronize up to `height`
    ChainSyncTimeOut((u32, u32, Duration)), // (current_height, target_height, timeout)

    /// Timed out whilst waiting for the [`Node`]'s to connect to each other.
    ConnectionTimeout(Duration),
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;

        match self {
            BinaryPathNotAbsolute { bin_name, path } => write!(f, "The `{}` binary path is not absolute (path={})", bin_name, path),
            BinaryPathNotFile { bin_name, path } => write!(f, "The `{}` binary path is not a file (path={})", bin_name, path),
            BinaryNotFound((bin_name, path)) => write!(f, "The `{}` binary was not found at the expected location={}", bin_name, path.display()),
            FailedToSpawn(err) => write!(f, "Failed to spawn a process for the node: {err:?}"),
            ExhaustedNodeBuildingRetries(retries) => write!(f, "Failed to instantiate the node after {} attempts", retries),
            FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err:?}"),
            Io(err) => write!(f, "I/O Error: {err:?}"),
            JsonRpc(err) => write!(f, "JSON-RPC Error: {err:?}"),
            PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            BothDirsSpecified => write!(f, "Both `tempdir` and `workdir` were specified. You must choose one and only one"),
            UnresponsiveBitcoinD(err) => write!(f, "`BitcoinD` is unresponsive to JSON-RPC calls: {err:?}"),
            UnresponsiveUtreexoD(err) => write!(f, "`UtreexoD` is unresponsive to JSON-RPC calls: {err:?}"),
            CookieFileTimeout(cookie_path) => write!(f, "Timed out whilst waiting for the cookie={} to be generated", cookie_path.display()),
            RpcClientSetupTimeout => write!(f, "Timed out whilst waiting for the JSON-RPC client to be ready"),
            UnexpectedResponse(err) => write!(f, "Received an unexpected response from the JSON-RPC server: {err:?}"),
            ChainSyncTimeOut((target_height, current_height, timeout)) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the node's chain to synchronize to height={} (current height={})",
                target_height, current_height, timeout.as_secs()
            ),
            ConnectionTimeout(timeout) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the nodes to connect to each other",
                timeout.as_secs()
            ),
        }
    }
}

impl error::Error for Error {}
