//! # Error
//!
//! Error types returned by [`Node`](crate::node::Node)
//! and indexer process management helpers.

use core::error;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;
use std::path::PathBuf;

/// Errors returned by node and indexer process management helpers.
#[derive(Debug)]
pub enum Error {
    /// The binary path is not absolute.
    BinaryPathNotAbsolute {
        /// Name of the binary whose path was rejected.
        bin_name: String,
        /// Rejected filesystem path.
        path: String,
    },

    /// The binary path is not a file.
    BinaryPathNotFile {
        /// Name of the binary whose path was rejected.
        bin_name: String,
        /// Rejected filesystem path.
        path: String,
    },

    /// The binary was not found at the expected location.
    BinaryNotFound((String, PathBuf)),

    /// Failed to spawn a [process](std::process::Child) for a [`Node`](crate::node::Node) or Electrum Server.
    FailedToSpawn(std::io::Error),

    /// Failed to instantiate a node or indexer after [`crate::SPAWN_ATTEMPTS`] attempts.
    ExhaustedNodeBuildingAttempts(u8),

    /// Failed to stop [`crate::bitcoind::BitcoinD`] or [`crate::utreexod::UtreexoD`] over JSON-RPC (e.g. `bitcoin-cli -regtest stop`).
    FailedToStop(corepc_client::client_sync::Error),

    /// I/O errors.
    Io(std::io::Error),

    /// JSON-RPC Errors.
    JsonRpc(corepc_client::client_sync::Error),

    /// Timed out whilst waiting for peer connection to succeed.
    PeerConnectionTimeout((SocketAddr, SocketAddr)),

    /// Both `tmpdir` and `staticdir` were specified.
    BothDirsSpecified,

    /// A raw CLI argument conflicts with a typed node configuration field.
    ConflictingNodeArgument(String),

    /// A raw CLI argument conflicts with typed or dynamic indexer configuration.
    ConflictingIndexerArgument(String),

    /// Typed node configuration contains an unsupported combination or value.
    InvalidNodeConfiguration(String),

    /// Indexer configuration is incompatible with its backing node.
    InvalidIndexerConfiguration(String),

    /// [`crate::bitcoind::BitcoinD`] is unresponsive (it's probably not running).
    #[cfg(feature = "bitcoind")]
    UnresponsiveBitcoinD(corepc_client::client_sync::Error),

    /// [`crate::utreexod::UtreexoD`] is unresponsive (it's probably not running).
    #[cfg(feature = "utreexod")]
    UnresponsiveUtreexoD(corepc_client::client_sync::Error),

    /// [`crate::electrsd::ElectrsD`] is unresponsive (it's probably not running).
    #[cfg(feature = "electrs")]
    UnresponsiveElectrsD(electrum_client::Error),

    /// [`crate::electrumxd::ElectrumxD`] is unresponsive (it's probably not running).
    #[cfg(feature = "electrumx")]
    UnresponsiveElectrumxD(electrum_client::Error),

    /// Timed out whilst waiting for [`crate::electrsd::ElectrsD`] to index expected data.
    #[cfg(feature = "electrs")]
    ElectrsDIndexTimeout((String, Duration)),

    /// Timed out whilst waiting for [`crate::electrumxd::ElectrumxD`] to index expected data.
    #[cfg(feature = "electrumx")]
    ElectrumxDIndexTimeout((String, Duration)),

    /// Timed out whilst waiting for the JSON-RPC client to be ready.
    RpcClientSetupTimeout,

    /// Received an unexpected response from the JSON-RPC server
    UnexpectedResponse(String),

    /// Timed out whilst waiting for the [`Node`](crate::node::Node)'s chain to synchronize up to `height`
    ChainSyncTimeOut((u32, u32, Duration)), // (target_height, current_height, timeout)

    /// Timed out whilst waiting for the [`Node`](crate::node::Node)s to connect to each other.
    ConnectionTimeout(Duration),
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryPathNotAbsolute { bin_name, path } => write!(f, "The `{}` binary path is not absolute (path={})", bin_name, path),
            Self::BinaryPathNotFile { bin_name, path } => write!(f, "The `{}` binary path is not a file (path={})", bin_name, path),
            Self::BinaryNotFound((bin_name, path)) => write!(f, "The `{}` binary was not found at the expected location={}", bin_name, path.display()),
            Self::FailedToSpawn(err) => write!(f, "Failed to spawn a process: {err:?}"),
            Self::ExhaustedNodeBuildingAttempts(retries) => write!(f, "Failed to instantiate the process after {} attempts", retries),
            Self::FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err:?}"),
            Self::Io(err) => write!(f, "I/O Error: {err:?}"),
            Self::JsonRpc(err) => write!(f, "JSON-RPC Error: {err:?}"),
            Self::PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            Self::BothDirsSpecified => write!(f, "Both `tmpdir` and `staticdir` were specified. You must choose one or neither"),
            Self::ConflictingNodeArgument(arg) => write!(f, "Raw node argument conflicts with typed configuration: {arg}"),
            Self::ConflictingIndexerArgument(arg) => write!(f, "Raw indexer argument conflicts with typed or dynamic configuration: {arg}"),
            Self::InvalidNodeConfiguration(description) => write!(f, "Invalid node configuration: {description}"),
            Self::InvalidIndexerConfiguration(description) => write!(f, "Invalid indexer configuration: {description}"),
            #[cfg(feature = "bitcoind")]
            Self::UnresponsiveBitcoinD(err) => write!(f, "`BitcoinD` is unresponsive to JSON-RPC calls: {err:?}"),
            #[cfg(feature = "utreexod")]
            Self::UnresponsiveUtreexoD(err) => write!(f, "`UtreexoD` is unresponsive to JSON-RPC calls: {err:?}"),
            #[cfg(feature = "electrs")]
            Self::UnresponsiveElectrsD(err) => write!(f, "`ElectrsD` is unresponsive to Electrum requests: {err:?}"),
            #[cfg(feature = "electrumx")]
            Self::UnresponsiveElectrumxD(err) => write!(f, "`ElectrumxD` is unresponsive to Electrum requests: {err:?}"),
            #[cfg(feature = "electrs")]
            Self::ElectrsDIndexTimeout((description, timeout)) => write!(f, "Timed out after {} seconds whilst waiting for `ElectrsD` to index {description}", timeout.as_secs()),
            #[cfg(feature = "electrumx")]
            Self::ElectrumxDIndexTimeout((description, timeout)) => write!(f, "Timed out after {} seconds whilst waiting for `ElectrumxD` to index {description}", timeout.as_secs()),
            Self::RpcClientSetupTimeout => write!(f, "Timed out whilst waiting for the JSON-RPC client to be ready"),
            Self::UnexpectedResponse(err) => write!(f, "Received an unexpected response from the JSON-RPC server: {err:?}"),
            Self::ChainSyncTimeOut((target_height, current_height, timeout)) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the node's chain to synchronize to height={} (current height={})",
                timeout.as_secs(), target_height, current_height
            ),
            Self::ConnectionTimeout(timeout) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the nodes to connect to each other",
                timeout.as_secs()
            ),
        }
    }
}
impl error::Error for Error {}
