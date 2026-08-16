// SPDX-License-Identifier: MIT OR Apache-2.0

//! Common interfaces and operations for Bitcoin [`Node`] implementations.
//!
//! The [`Node`] trait defines the operations that each implementation supplies.
//! [`NodeArgs`] contains configuration that is common to all [`Node`] implementations.
//! The connection and wait functions coordinate two or more enabled [`Node`] implementations.
//!
//! Enable the `bitcoind`, `florestad`, or `utreexod` features to use the selected implementation.
//!
//! [`Node`]: crate::node::Node

#[cfg(feature = "bitcoind")]
pub mod bitcoind;
pub mod error;
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
#[cfg(halfin_node)]
use std::thread::sleep;
#[cfg(halfin_node)]
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
#[cfg(halfin_node)]
use tracing::debug;
#[cfg(halfin_node)]
use tracing::info;

pub use self::error::NodeClientError;
pub use self::error::NodeError;
#[cfg(halfin_node)]
use crate::CONNECTION_INTERVAL;
#[cfg(halfin_node)]
use crate::CONNECTION_TIMEOUT;
use crate::POLL_INTERVAL;
use crate::WAIT_TIMEOUT;
use crate::error::Error;

/// Minimum automatic pruning target for all supported daemons, in MiB.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const MIN_PRUNE_TARGET_MIB: u64 = 550;

/// File name for RPC authentication cookies that `halfin` creates.
#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) const RPC_COOKIE_FILE_NAME: &str = ".cookie";

/// User name in RPC authentication cookies that `halfin` creates.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const RPC_USER: &str = "__cookie__";

/// Password in RPC authentication cookies that `halfin` creates.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) const RPC_PASS: &str = "halfin";

/// Arguments shared by the supported [`Node`] implementations.
///
/// This type does not implement [`Default`]. Each daemon configuration supplies its default values.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct NodeArgs {
    /// Bitcoin [`Network`] for the [`Node`].
    pub network: Network,
    /// P2P peers that the [`Node`] connects to exclusively.
    pub fixed_peers: Vec<SocketAddr>,
    /// Enables the BIP-0324 `P2Pv2` transport.
    pub v2_transport: bool,
    /// Builds the compact block filter index.
    pub cbf_index: bool,
    /// Block-pruning behavior.
    pub prune: PruneMode,
    /// Builds the full transaction index.
    pub txindex: bool,
}

/// Block-pruning behavior for a [`Node`].
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
    /// Configuration type of this [`Node`] implementation.
    type Config: AsRef<NodeArgs>;

    /// Human-readable name of the [`Node`].
    fn get_name() -> &'static str;

    /// Binary name of the [`Node`].
    fn get_bin_name() -> &'static str;

    /// Return the complete configuration used to start this [`Node`].
    fn get_config(&self) -> &Self::Config;

    /// Return the effective runtime data directory of the [`Node`].
    ///
    /// [`Indexer`](crate::indexer::Indexer) backends must put RPC credentials in `.cookie` in this
    /// directory. The credentials must use the `user:password` format.
    fn get_working_directory(&self) -> PathBuf;

    /// Return the JSON-RPC listener address of the [`Node`].
    fn get_rpc_socket(&self) -> SocketAddr;

    /// Mine `count` blocks and return their hashes.
    ///
    /// # Errors
    ///
    /// Returns an error if block generation fails.
    ///
    /// An implementation returns [`NodeError::UnsupportedCommand`] if its daemon cannot generate
    /// blocks.
    fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error>;

    /// Return the current chain height of the [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the [`Node`] cannot report its current chain height.
    fn get_chain_tip(&self) -> Result<u32, Error>;

    /// Return the current compact block filter height of the [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the [`Node`] cannot report its current compact-filter height.
    ///
    /// An implementation returns [`NodeError::UnsupportedCommand`] if its daemon cannot report
    /// compact filter progress.
    fn get_filter_tip(&self) -> Result<u32, Error>;

    /// Get the [`BlockHash`] of the block at `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot get or parse the block hash.
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error>;

    /// Call a JSON-RPC `method` with the specified `args` list.
    ///
    /// This method does not deserialize the response.
    /// Parse the returned [`Value`](serde_json::Value) into the required type.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON-RPC call fails.
    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error>;

    /// Return the inbound P2P [`SocketAddr`] of the [`Node`].
    ///
    /// # Panics
    ///
    /// An implementation may panic when its daemon has no inbound P2P listener.
    fn get_p2p_socket(&self) -> SocketAddr;

    /// Check whether the [`Node`] has a peer with the specified [`SocketAddr`].
    ///
    /// # Errors
    ///
    /// Returns an error if the [`Node`] cannot query its peer state.
    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error>;

    /// Connect this [`Node`] to a peer at `socket` over P2P.
    ///
    /// # Errors
    ///
    /// Returns an error if the [`Node`] cannot add or confirm the peer connection.
    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error>;

    /// Return the peer count of this [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the [`Node`] cannot query its peer count.
    fn get_peer_count(&self) -> Result<u32, Error>;

    /// Interval between `get_chain_tip` RPC calls.
    ///
    /// Defaults to [`POLL_INTERVAL`].
    ///
    /// Override this value if a [`Node`] needs more time between RPC calls.
    fn poll_interval() -> Duration {
        POLL_INTERVAL
    }

    /// Maximum time that `wait_for_height` polls the [`Node`].
    ///
    /// Defaults to [`WAIT_TIMEOUT`].
    ///
    /// Override this value if a [`Node`] needs more time to process blocks.
    /// For example, [`UtreexoD`](crate::node::utreexod::UtreexoD) needs more time to build the
    /// Merkle forest.
    fn wait_timeout() -> Duration {
        WAIT_TIMEOUT
    }
}

/// Connect [`Node`] A to [`Node`] B.
///
/// The order is important if a [`Node`] has no inbound P2P listener.
/// You can use `FlorestaD` as `a`, but not as `b`.
///
/// # Errors
///
/// Returns an error if [`Node`] A cannot add or confirm the peer before [`CONNECTION_TIMEOUT`].
///
/// # Panics
///
/// Panics if [`Node`] B is `FlorestaD` because it has no inbound P2P listener.
#[cfg(halfin_node)]
pub fn connect<A: Node, B: Node>(a: &A, b: &B) -> Result<(), Error> {
    connect_with_timeout(a, b, CONNECTION_TIMEOUT, CONNECTION_INTERVAL)
}

/// Connect node `a` to node `b` with explicit wait timing.
#[cfg(halfin_node)]
fn connect_with_timeout<A: Node, B: Node>(
    a: &A,
    b: &B,
    timeout: Duration,
    interval: Duration,
) -> Result<(), Error> {
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
    while start.elapsed() < timeout {
        if is_connected()? {
            // Allow time for v2 transport negotiation to settle,
            // or for v1 fallback to complete if v2 fails, then re-verify.
            sleep(interval * 4);
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
        sleep(interval);
    }

    Err(NodeError::ConnectionTimeout(timeout).into())
}

/// Connect [`Node`] A to [`Node`] B and wait for them to synchronize chains.
///
/// # Errors
///
/// Returns an error if the [`Node`] implementations cannot connect or report their chain heights.
/// Returns an error if a [`Node`] does not reach the shared height before its timeout.
///
/// # Panics
///
/// Panics if [`Node`] B is `FlorestaD` because it has no inbound P2P listener.
#[cfg(halfin_node)]
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
/// Returns an error if the [`Node`] does not reach `height` within [`Node::wait_timeout`].
#[cfg(halfin_node)]
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
    Err(NodeError::ChainSyncTimeout((height, curr_height, N::wait_timeout())).into())
}

/// Poll a [`Node`] until its chain reaches `height` with a custom `timeout`.
///
/// # Errors
///
/// Returns an error if the [`Node`] does not reach `height` within `timeout`.
#[cfg(halfin_node)]
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
    Err(NodeError::ChainSyncTimeout((height, curr_height, timeout)).into())
}

/// Poll a [`Node`] until its compact block filters reach `height`.
///
/// # Errors
///
/// Returns an error if the [`Node`] does not reach `filter_height` within [`Node::wait_timeout`].
///
/// # Panics
///
/// Panics if the [`Node`] does not supply compact filter progress.
#[cfg(halfin_node)]
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
    Err(NodeError::ChainSyncTimeout((filter_height, curr_filter_height, N::wait_timeout())).into())
}

/// Validate constraints common to every [`Node`] implementation.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(crate) fn validate_node_arguments(args: &NodeArgs) -> Result<(), Error> {
    if let PruneMode::Automatic(target_mib) = args.prune {
        if target_mib < MIN_PRUNE_TARGET_MIB {
            return Err(NodeError::InvalidConfiguration(format!(
                "automatic pruning target must be at least {MIN_PRUNE_TARGET_MIB} MiB (got {target_mib} MiB)"
            ))
            .into());
        }
    }

    if args.prune != PruneMode::Disabled && args.txindex {
        return Err(NodeError::InvalidConfiguration(
            "pruning and transaction indexing are mutually exclusive".to_string(),
        )
        .into());
    }

    Ok(())
}

/// Write the RPC cookie shared by a [`Node`] and its [`Indexer`](crate::indexer::Indexer)
/// implementations.
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

#[cfg(all(test, halfin_node))]
mod test;
