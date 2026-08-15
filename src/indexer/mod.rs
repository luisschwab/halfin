// SPDX-License-Identifier: MIT OR Apache-2.0

//! Common interfaces and operations for Electrum [`Indexer`] implementations.
//!
//! The [`Indexer`] trait defines the operations that each implementation supplies.
//! The shared functions validate a backing [`Node`] and its RPC cookie.
//!
//! Enable the `electrs` or `electrumx` features to use the selected implementation.
//!
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

#[cfg(feature = "electrs")]
pub mod electrsd;
#[cfg(feature = "electrumx")]
pub mod electrumxd;
pub mod error;

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

pub use self::error::IndexerError;
use crate::Error;
use crate::node::Node;
use crate::node::RPC_COOKIE_FILE_NAME;

/// Reject a [`Node`] implementation that no [`Indexer`] supports.
pub(crate) fn validate_backend<N: Node>() -> Result<(), Error> {
    if matches!(N::get_name(), "FlorestaD" | "UtreexoD") {
        return Err(IndexerError::UnsupportedBackend {
            node: N::get_name(),
        }
        .into());
    }
    Ok(())
}

/// Make sure that an [`Indexer`] can use a [`Node`].
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

/// Read and validate the RPC cookie of the [`Node`].
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
        return Err(IndexerError::InvalidConfiguration(
            "backing node RPC cookie must contain user:password credentials".to_string(),
        )
        .into());
    }

    Ok((cookie_file, credentials))
}

/// Common interface across all Electrum [`Indexer`] implementations.
pub trait Indexer {
    /// Configuration type of this [`Indexer`] implementation.
    type Config;

    /// Human-readable name of the [`Indexer`].
    fn get_name() -> &'static str;

    /// Binary name of the [`Indexer`].
    fn get_bin_name() -> &'static str;

    /// Tell the [`Indexer`] to check its [`Node`] for new data.
    ///
    /// # Errors
    ///
    /// Returns an error if the trigger for the implementation fails.
    fn trigger(&self) -> Result<(), Error>;

    /// Stop the [`Indexer`] process and wait for it to exit.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot wait for the child process.
    fn stop(&mut self) -> Result<ExitStatus, Error>;

    /// Return the operating system process ID of the [`Indexer`].
    fn get_pid(&self) -> u32;

    /// Return the data directory of the [`Indexer`].
    fn get_working_directory(&self) -> PathBuf;

    /// Return the complete configuration used to start this [`Indexer`].
    fn get_config(&self) -> &Self::Config;

    /// Return a reference to the Electrum client of the [`Indexer`].
    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream>;

    /// Return the Electrum RPC [`SocketAddr`] of the [`Indexer`].
    fn get_electrum_socket(&self) -> SocketAddr;

    /// Return the Electrum RPC URL of the [`Indexer`].
    fn get_electrum_url(&self) -> String;

    /// Wait until the [`Indexer`] reaches the chain tip of the specified [`Node`].
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot query the [`Node`].
    /// Returns an error if the [`Indexer`] does not reach the [`Node`] tip before the timeout.
    fn wait_until_caught_up(
        &self,
        node: &impl Node,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;

    /// Wait until the [`Indexer`] reaches `exp_height` and `exp_hash`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot query the [`Indexer`].
    /// Returns an error if the [`Indexer`] does not reach the expected tip before the timeout.
    fn wait_until_tip(
        &self,
        exp_height: u32,
        exp_hash: BlockHash,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;

    /// Wait until an unconfirmed transaction appears in the history of `spk`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot query the [`Indexer`].
    /// Returns an error if the transaction does not appear before the timeout.
    fn wait_until_mempool_tx(
        &self,
        spk: &Script,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<(), Error>;
}
