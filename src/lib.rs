//! # halfin
//!
//! A bitcoin node running utility for integration testing.
//!
//! > A {regtest} bitcoin node runner 🏃‍♂️
//!
//! This crate makes it simple to run regtest [`bitcoind`], [`utreexod`],
//! and [`electrs`] instances from Rust code, useful in integration test contexts.
//!
//! ## Supported Implementations
//!
//! | Implementation | Version   | Feature Flag     | Default Feature |
//! |----------------|-----------|------------------|-----------------|
//! | [`bitcoind`]   | `v31.0`   | `bitcoind`       | Yes             |
//! |                |           |                  |                 |
//! | [`electrs`]    | `v0.11.1` | `electrs`        | No              |
//! |                |           |                  |                 |
//! | [`utreexod`]   | `v0.6.0`  | `utreexod`       | Yes             |
//!
//! ## Example
//!
//! ```rust,ignore
//! use halfin::bitcoind::BitcoinD;
//! use halfin::connect;
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
//! [`electrs`]: <https://github.com/romanz/electrs>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>

use core::net::Ipv4Addr;
use core::net::SocketAddr;
use corepc_client::bitcoin::BlockHash;
pub use error::Error;
#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;
use tracing::debug;
use tracing::info;
#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
use tracing::trace;

pub use serde_json;

#[allow(unused)]
#[cfg(feature = "bitcoind")]
pub(crate) use bitcoind::BitcoinD;
#[allow(unused)]
#[cfg(feature = "electrs")]
pub(crate) use electrsd::ElectrsD;
#[allow(unused)]
#[cfg(feature = "utreexod")]
pub(crate) use utreexod::UtreexoD;

#[cfg(feature = "bitcoind")]
pub mod bitcoind;
#[cfg(feature = "electrs")]
pub mod electrsd;
pub mod error;
#[cfg(feature = "utreexod")]
pub mod utreexod;

/// IPv4 localhost address.
const IPV4_LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// Maximum number of attempts at instantiating a [`Node`] process.
pub const NODE_BUILDING_ATTEMPTS: u8 = 5;

/// Period between attempts at instantiating a [`Node`] process.
pub const NODE_BUILDING_INTERVAL: Duration = Duration::from_millis(500);

/// Period between polls for [`connect`] and [`wait_for_height`].
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Timeout for [`connect`] and [`wait_for_height`].
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Period between successive attempts of [`Node`] connection.
pub const CONNECTION_INTERVAL: Duration = Duration::from_millis(150);

/// Timeout for [`Node`] connection.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Common interface across all node implementations ([`BitcoinD`]/[`UtreexoD`]).
pub trait Node {
    /// The [`Node`]'s human-readable name.
    fn get_name() -> &'static str;

    /// The [`Node`]'s binary name.
    fn get_bin_name() -> &'static str;

    /// Get the [`Node`]'s current chain height.
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot report its current chain height.
    fn get_chain_tip(&self) -> Result<u32, Error>;

    /// Get the [`Node`]'s current CBF height.
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot report its current compact-filter height.
    fn get_filter_tip(&self) -> Result<u32, Error>;

    /// Get the [`BlockHash`] of the block at `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the block hash cannot be fetched or parsed.
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error>;

    /// Call a JSON-RPC `method` with the given `args` list.
    ///
    /// Response deserialization is not implemented for this method.
    ///
    /// It's up to the caller to parse the returned
    /// [`Value`](serde_json::Value) into a meaningful type.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails.
    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error>;

    /// Get the [`Node`]'s P2P [`SocketAddr`].
    fn get_p2p_socket(&self) -> SocketAddr;

    /// Check whether the [`Node`] is connected to a peer with a specific [`SocketAddr`].
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot query its peer state.
    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error>;

    /// Connect this [`Node`] to a peer at `socket` over P2P.
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot add or confirm the peer connection.
    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error>;

    /// Get this [`Node`]' s peer count.
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot query its peer count.
    fn get_peer_count(&self) -> Result<u32, Error>;

    /// How long to sleep between `get_chain_tip` RPC calls.
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
///
/// # Errors
///
/// Returns an error if either node cannot add or confirm the peer connection
/// before [`CONNECTION_TIMEOUT`].
pub fn connect<A: Node, B: Node>(a: &A, b: &B) -> Result<(), Error> {
    let socket_a = a.get_p2p_socket();
    let socket_b = b.get_p2p_socket();

    debug!(
        "Connecting {} at socket={} to {} at socket={}",
        A::get_bin_name(),
        socket_a,
        B::get_bin_name(),
        socket_b
    );

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
                info!(
                    "Connecting {} at socket={} to {} at socket={}",
                    A::get_bin_name(),
                    socket_a,
                    B::get_bin_name(),
                    socket_b
                );

                return Ok(());
            }
        }
        thread::sleep(CONNECTION_INTERVAL);
    }

    Err(Error::ConnectionTimeout(CONNECTION_TIMEOUT))
}

/// Connect [`Node`] A to [`Node`] B and wait for them to synchronize chains.
///
/// # Errors
///
/// Returns an error if the nodes cannot connect, either chain height cannot be
/// queried, or either node fails to reach the shared height before its timeout.
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
/// # Errors
///
/// Returns an error if the node does not reach `height` within [`Node::wait_timeout`].
pub fn wait_for_height<N: Node>(node: &N, height: u32) -> Result<(), Error> {
    debug!("Waiting for {} to reach height={}", N::get_name(), height);

    let start = Instant::now();
    while start.elapsed() < N::wait_timeout() {
        if node.get_chain_tip().unwrap_or(0) >= height {
            info!("{} to reached height={}", N::get_name(), height);

            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_chain_tip().unwrap_or(0);
    Err(Error::ChainSyncTimeOut((
        height,
        curr_height,
        N::wait_timeout(),
    )))
}

/// Poll a [`Node`] until its chain reaches `height` with a custom `timeout`.
///
/// # Errors
///
/// Returns an error if the node does not reach `height` within `timeout`.
pub fn wait_for_height_with_timeout<N: Node>(
    node: &N,
    height: u32,
    timeout: Duration,
) -> Result<(), Error> {
    debug!(
        "Waiting for {} to reach height={} with timeout={}seconds)",
        N::get_name(),
        height,
        timeout.as_secs()
    );

    let start = Instant::now();
    while start.elapsed() < timeout {
        if node.get_chain_tip().unwrap_or(0) >= height {
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_height = node.get_chain_tip().unwrap_or(0);
    Err(Error::ChainSyncTimeOut((height, curr_height, timeout)))
}

/// Poll a [`Node`] until its Compact Block Filters reach `height`.
///
/// # Errors
///
/// Returns an error if the node does not reach `filter_height` within [`Node::wait_timeout`].
pub fn wait_for_filter_height<N: Node>(node: &N, filter_height: u32) -> Result<(), Error> {
    debug!(
        "Waiting for {} to reach filter_height={}",
        N::get_name(),
        filter_height
    );

    let start = Instant::now();
    while start.elapsed() < N::wait_timeout() {
        if node.get_filter_tip().unwrap_or(0) >= filter_height {
            info!(
                "{} to reached filter_height={}",
                N::get_name(),
                filter_height
            );
            return Ok(());
        }
        thread::sleep(N::poll_interval());
    }

    let curr_filter_height = node.get_filter_tip().unwrap_or(0);
    Err(Error::ChainSyncTimeOut((
        filter_height,
        curr_filter_height,
        N::wait_timeout(),
    )))
}

/// Spawn a background thread that reads `reader` line by line and re-emits
/// each line as a [`trace!`] event, prefixed with `source`.
///
/// Used to pipe a child [`BitcoinD`]/[`UtreexoD`]/[`ElectrsD`] process `stdout`/`stderr`
/// into [`tracing`]. The thread exits on EOF, which happens when the process
/// dies and its pipe is closed.
#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
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
/// # Panics
///
/// Panics if the OS cannot bind a localhost ephemeral port or report the local socket address.
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
