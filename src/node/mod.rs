// SPDX-License-Identifier: MIT OR Apache-2.0

//! # `Node` trait
//!
//! This module implements the [`Node`] trait, with common methods
//! and utilities across all Bitcoin [`Node`] implementations.

#[cfg(feature = "bitcoind")]
pub mod bitcoind;
#[cfg(feature = "florestad")]
pub mod florestad;
#[cfg(feature = "utreexod")]
pub mod utreexod;

use core::net::SocketAddr;
use core::time::Duration;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::fs::OpenOptions;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::io::Write;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::path::Path;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use tracing::debug;
use tracing::info;

use crate::CONNECTION_INTERVAL;
use crate::CONNECTION_TIMEOUT;
use crate::POLL_INTERVAL;
use crate::WAIT_TIMEOUT;
use crate::error::Error;

/// Minimum automatic-pruning target supported by both daemons, in MiB.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const MIN_PRUNE_TARGET_MIB: u64 = 550;

/// Filename used for halfin-owned RPC authentication cookies.
#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) const RPC_COOKIE_FILE_NAME: &str = ".cookie";

/// Username stored in halfin-owned RPC authentication cookies.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const RPC_USER: &str = "__cookie__";

/// Password stored in halfin-owned RPC authentication cookies.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const RPC_PASS: &str = "halfin";

/// Arguments shared by the supported [`Node`] implementations.
///
/// This type intentionally does not implement [`Default`]. Each daemon's
/// configuration chooses defaults appropriate for that implementation.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct NodeArgs {
    /// Bitcoin [`Network`] to run on.
    pub network: Network,
    /// Whether to enable BIP-0324 `P2Pv2` transport.
    pub v2_transport: bool,
    /// Whether to build the compact block-filter index.
    pub cbf_index: bool,
    /// Block-pruning behavior.
    pub prune: PruneMode,
    /// Whether to build the full transaction index.
    pub txindex: bool,
}

/// Block-pruning behavior for a node.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PruneMode {
    /// Retain all block data.
    Disabled,
    /// Let the caller prune blocks explicitly through RPC.
    Manual,
    /// Automatically prune block data to approximately the target size in MiB.
    Automatic(u64),
}

/// Common interface across all [`Node`] implementations.
pub trait Node {
    /// Concrete configuration type retained by this node implementation.
    type Config: AsRef<NodeArgs>;

    /// The [`Node`]'s human-readable name.
    fn get_name() -> &'static str;

    /// The [`Node`]'s binary name.
    fn get_bin_name() -> &'static str;

    /// Return the complete configuration used to start this node.
    fn get_config(&self) -> &Self::Config;

    /// Return the node's effective runtime data directory.
    ///
    /// Implementations intended for use as indexer backends must expose RPC
    /// credentials at `.cookie` in this directory, encoded as `user:password`.
    fn get_working_directory(&self) -> PathBuf;

    /// Return the node's JSON-RPC listener address.
    fn get_rpc_socket(&self) -> SocketAddr;

    /// Mine `count` blocks and return their hashes.
    ///
    /// # Errors
    ///
    /// Returns an error if block generation fails.
    ///
    /// Implementations whose daemon cannot generate blocks return
    /// [`Error::UnsupportedCommand`].
    fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error>;

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
    ///
    /// Implementations whose daemon cannot report compact-filter progress
    /// return [`Error::UnsupportedCommand`].
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

    /// Get the [`Node`]'s inbound P2P [`SocketAddr`].
    ///
    /// # Panics
    ///
    /// An implementation may panic when its daemon has no inbound P2P listener.
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
    /// (e.g. [`UtreexoD`](crate::node::utreexod::UtreexoD) needs more time to build the Merkle
    /// forest).
    fn wait_timeout() -> Duration {
        WAIT_TIMEOUT
    }
}

/// Connect [`Node`] A to [`Node`] B.
///
/// The ordering is significant for nodes without an inbound P2P listener. In
/// particular, `FlorestaD` may be used as `a` but not as `b`.
///
/// # Errors
///
/// Returns an error if node A cannot add or confirm the peer connection before
/// [`CONNECTION_TIMEOUT`].
///
/// # Panics
///
/// Panics if node B is `FlorestaD`, which does not provide an inbound P2P listener.
pub fn connect<A: Node, B: Node>(a: &A, b: &B) -> Result<(), Error> {
    assert_ne!(
        B::get_name(),
        "FlorestaD",
        "FlorestaD cannot be node B because it has no inbound P2P listener"
    );

    let socket_b = b.get_p2p_socket();

    debug!(
        "Connecting {} outbound to {} at socket={}",
        A::get_bin_name(),
        B::get_bin_name(),
        socket_b
    );

    a.add_peer(socket_b)?;

    let is_connected = || a.has_peer(socket_b);

    // The outbound node can always identify its peer by the peer's listening
    // socket, including when the peer does not expose inbound peer metadata.
    let start = Instant::now();
    while start.elapsed() < CONNECTION_TIMEOUT {
        if is_connected()? {
            // Allow time for v2 transport negotiation to settle,
            // or for v1 fallback to complete if v2 fails, then re-verify.
            sleep(CONNECTION_INTERVAL * 4);
            if is_connected()? {
                info!(
                    "Connected {} outbound to {} at socket={}",
                    A::get_bin_name(),
                    B::get_bin_name(),
                    socket_b
                );

                return Ok(());
            }
        }
        sleep(CONNECTION_INTERVAL);
    }

    Err(Error::ConnectionTimeout(CONNECTION_TIMEOUT))
}

/// Connect [`Node`] A to [`Node`] B and wait for them to synchronize chains.
///
/// # Errors
///
/// Returns an error if the nodes cannot connect, either chain height cannot be
/// queried, or either node fails to reach the shared height before its timeout.
///
/// # Panics
///
/// Panics if node B is `FlorestaD`, which does not provide an inbound P2P listener.
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
        sleep(N::poll_interval());
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
        sleep(N::poll_interval());
    }

    let curr_height = node.get_chain_tip().unwrap_or(0);
    Err(Error::ChainSyncTimeOut((height, curr_height, timeout)))
}

/// Poll a [`Node`] until its Compact Block Filters reach `height`.
///
/// # Errors
///
/// Returns an error if the node does not reach `filter_height` within [`Node::wait_timeout`].
///
/// # Panics
///
/// Panics if the node does not expose compact-filter progress.
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
        sleep(N::poll_interval());
    }

    let curr_filter_height = node.get_filter_tip().unwrap_or(0);
    Err(Error::ChainSyncTimeOut((
        filter_height,
        curr_filter_height,
        N::wait_timeout(),
    )))
}

/// Validate constraints common to every node implementation.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) fn validate_node_arguments(args: &NodeArgs) -> Result<(), Error> {
    if let PruneMode::Automatic(target_mib) = args.prune {
        if target_mib < MIN_PRUNE_TARGET_MIB {
            return Err(Error::InvalidNodeConfiguration(format!(
                "automatic pruning target must be at least {MIN_PRUNE_TARGET_MIB} MiB (got {target_mib} MiB)"
            )));
        }
    }

    if args.prune != PruneMode::Disabled && args.txindex {
        return Err(Error::InvalidNodeConfiguration(
            "pruning and transaction indexing are mutually exclusive".to_string(),
        ));
    }

    Ok(())
}

/// Write the RPC cookie shared by a node and its indexers.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) fn write_rpc_cookie(data_dir: &Path) -> Result<PathBuf, Error> {
    let cookie_file = data_dir.join(RPC_COOKIE_FILE_NAME);
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&cookie_file).map_err(Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(Error::Io)?;
    }
    write!(file, "{RPC_USER}:{RPC_PASS}").map_err(Error::Io)?;
    Ok(cookie_file)
}
