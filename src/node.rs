// SPDX-License-Identifier: MIT OR Apache-2.0

//! # `Node` trait
//!
//! This module implements the `Node` trait, with common methods
//! and utilities across all Bitcoin node implementations.

use crate::CONNECTION_INTERVAL;
use crate::CONNECTION_TIMEOUT;
use crate::POLL_INTERVAL;
use crate::WAIT_TIMEOUT;
use crate::error::Error;
use core::net::SocketAddr;
use core::time::Duration;
use corepc_client::bitcoin::BlockHash;
use std::thread::sleep;
use std::time::Instant;
use tracing::debug;
use tracing::info;

/// Common interface across all node implementations ([`BitcoinD`](crate::bitcoind::BitcoinD)/[`UtreexoD`](crate::utreexod::UtreexoD)).
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
    /// (e.g. [`UtreexoD`](crate::utreexod::UtreexoD) needs more time to build the Merkle forest).
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
            sleep(CONNECTION_INTERVAL * 4);
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
