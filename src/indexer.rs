// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared interface for Electrum indexer implementations.

use core::net::SocketAddr;
use core::time::Duration;
use std::fs;
use std::path::PathBuf;
use std::process::ExitStatus;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::Txid;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use tracing::debug;

use crate::Error;
use crate::node::Node;
use crate::node::RPC_COOKIE_FILE_NAME;

/// Reject backing node implementations unsupported by every indexer.
pub(crate) fn validate_backend<N: Node>() -> Result<(), Error> {
    if matches!(N::get_name(), "FlorestaD" | "UtreexoD") {
        return Err(Error::UnsupportedIndexerBackend {
            node: N::get_name(),
        });
    }
    Ok(())
}

/// Ensure a backing node is ready to be indexed.
pub(crate) fn ensure_backend_ready(
    node: &impl Node,
    network: Network,
    indexer_name: &str,
) -> Result<(), Error> {
    let blockchain_info = node.call("getblockchaininfo", &[])?;
    let initial_block_download = blockchain_info
        .get("initialblockdownload")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let blocks = blockchain_info
        .get("blocks")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    debug!(
        "{indexer_name}: checked backing node readiness initial_block_download={initial_block_download} blocks={blocks}"
    );

    if network == Network::Regtest && (initial_block_download || blocks == 0) {
        let _ = node.generate(1)?;
    }
    Ok(())
}

/// Read and validate the backing node's RPC cookie.
pub(crate) fn read_backend_cookie(node: &impl Node) -> Result<(PathBuf, String), Error> {
    let cookie_file = node.get_working_directory().join(RPC_COOKIE_FILE_NAME);
    let credentials = fs::read_to_string(&cookie_file)
        .map_err(Error::Io)?
        .trim()
        .to_string();
    let valid = credentials
        .split_once(':')
        .is_some_and(|(user, password)| !user.is_empty() && !password.is_empty());
    if !valid {
        return Err(Error::InvalidIndexerConfiguration(
            "backing node RPC cookie must contain user:password credentials".to_string(),
        ));
    }

    Ok((cookie_file, credentials))
}

/// Common interface across all Electrum [`Indexer`] implementations.
pub trait Indexer {
    /// Concrete configuration type retained by this indexer implementation.
    type Config;

    /// The indexer's human-readable name.
    fn get_name() -> &'static str;

    /// The indexer's binary name.
    fn get_bin_name() -> &'static str;

    /// Trigger the indexer to check its backing node for updated state.
    ///
    /// # Errors
    ///
    /// Returns an error if the implementation-specific trigger fails.
    fn trigger(&self) -> Result<(), Error>;

    /// Stop the indexer process and wait for it to exit.
    ///
    /// # Errors
    ///
    /// Returns an error if the child process cannot be waited on.
    fn stop(&mut self) -> Result<ExitStatus, Error>;

    /// Return the OS process ID of the running indexer.
    fn get_pid(&self) -> u32;

    /// Return the indexer's data directory.
    fn get_working_directory(&self) -> PathBuf;

    /// Return the complete configuration used to start this indexer.
    fn get_config(&self) -> &Self::Config;

    /// Return a reference to the indexer's Electrum client.
    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream>;

    /// Return the Electrum RPC [`SocketAddr`] the indexer is listening on.
    fn get_electrum_socket(&self) -> SocketAddr;

    /// Return the Electrum RPC URL for the indexer.
    fn get_electrum_url(&self) -> String;

    /// Wait until the indexer reaches the supplied [`Node`]'s chain tip.
    ///
    /// # Errors
    ///
    /// Returns an error if the backing node cannot be queried or the indexer
    /// does not catch up before the timeout.
    fn wait_until_caught_up(
        &self,
        node: &impl Node,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;

    /// Wait until the indexer reaches `exp_height` and `exp_hash`.
    ///
    /// # Errors
    ///
    /// Returns an error if the indexer cannot be queried or does not reach the
    /// expected tip before the timeout.
    fn wait_until_tip(
        &self,
        exp_height: u32,
        exp_hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;

    /// Wait until an unconfirmed transaction appears in `spk`'s history.
    ///
    /// # Errors
    ///
    /// Returns an error if the indexer cannot be queried or the transaction is
    /// not observed before the timeout.
    fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;
}
