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

    /// Both `tmpdir` and `workdir` were specified.
    BothDirsSpecified,

    /// [`crate::bitcoind::BitcoinD`] is unresponsive (it's probably not running).
    #[cfg(feature = "bitcoind")]
    UnresponsiveBitcoinD(corepc_client::client_sync::Error),

    /// [`crate::utreexod::UtreexoD`] is unresponsive (it's probably not running).
    #[cfg(feature = "utreexod")]
    UnresponsiveUtreexoD(corepc_client::client_sync::Error),

    /// [`crate::electrsd::ElectrsD`] is unresponsive (it's probably not running).
    #[cfg(feature = "electrs")]
    UnresponsiveElectrsD(electrum_client::Error),

    /// Timed out whilst waiting for [`crate::electrsd::ElectrsD`] to index expected data.
    #[cfg(feature = "electrs")]
    ElectrsDIndexTimeout((String, Duration)),

    /// Timed out whilst waiting for the cookie file to be generated.
    CookieFileTimeout(PathBuf),

    /// Timed out whilst waiting for the JSON-RPC client to be ready.
    RpcClientSetupTimeout,

    /// Received an unexpected response from the JSON-RPC server
    UnexpectedResponse(String),

    /// Timed out whilst waiting for the [`Node`](crate::node::Node)'s chain to synchronize up to `height`
    ChainSyncTimeOut((u32, u32, Duration)), // (current_height, target_height, timeout)

    /// Timed out whilst waiting for the [`Node`](crate::node::Node)'s to connect to each other.
    ConnectionTimeout(Duration),
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryPathNotAbsolute { bin_name, path } => write!(f, "The `{}` binary path is not absolute (path={})", bin_name, path),
            Self::BinaryPathNotFile { bin_name, path } => write!(f, "The `{}` binary path is not a file (path={})", bin_name, path),
            Self::BinaryNotFound((bin_name, path)) => write!(f, "The `{}` binary was not found at the expected location={}", bin_name, path.display()),
            Self::FailedToSpawn(err) => write!(f, "Failed to spawn a process for the node: {err:?}"),
            Self::ExhaustedNodeBuildingAttempts(retries) => write!(f, "Failed to instantiate the node after {} attempts", retries),
            Self::FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err:?}"),
            Self::Io(err) => write!(f, "I/O Error: {err:?}"),
            Self::JsonRpc(err) => write!(f, "JSON-RPC Error: {err:?}"),
            Self::PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            Self::BothDirsSpecified => write!(f, "Both `tempdir` and `workdir` were specified. You must choose one and only one"),
            #[cfg(feature = "bitcoind")]
            Self::UnresponsiveBitcoinD(err) => write!(f, "`BitcoinD` is unresponsive to JSON-RPC calls: {err:?}"),
            #[cfg(feature = "utreexod")]
            Self::UnresponsiveUtreexoD(err) => write!(f, "`UtreexoD` is unresponsive to JSON-RPC calls: {err:?}"),
            #[cfg(feature = "electrs")]
            Self::UnresponsiveElectrsD(err) => write!(f, "`ElectrsD` is unresponsive to Electrum requests: {err:?}"),
            #[cfg(feature = "electrs")]
            Self::ElectrsDIndexTimeout((description, timeout)) => write!(f, "Timed out after {} seconds whilst waiting for `ElectrsD` to index {description}", timeout.as_secs()),
            Self::CookieFileTimeout(cookie_path) => write!(f, "Timed out whilst waiting for the cookie={} to be generated", cookie_path.display()),
            Self::RpcClientSetupTimeout => write!(f, "Timed out whilst waiting for the JSON-RPC client to be ready"),
            Self::UnexpectedResponse(err) => write!(f, "Received an unexpected response from the JSON-RPC server: {err:?}"),
            Self::ChainSyncTimeOut((target_height, current_height, timeout)) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the node's chain to synchronize to height={} (current height={})",
                target_height, current_height, timeout.as_secs()
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
