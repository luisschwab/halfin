//! Errors produced by node operations.

use core::error;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;

/// Client errors that can make a node unresponsive.
#[derive(Debug)]
pub enum NodeClientError {
    /// A JSON-RPC client error.
    JsonRpc(corepc_client::client_sync::Error),

    /// An Electrum client error.
    #[cfg(feature = "florestad")]
    Electrum(electrum_client::Error),
}

impl fmt::Display for NodeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JsonRpc(err) => write!(f, "JSON-RPC client error: {err}"),
            #[cfg(feature = "florestad")]
            Self::Electrum(err) => write!(f, "Electrum client error: {err}"),
        }
    }
}

impl error::Error for NodeClientError {}

impl From<corepc_client::client_sync::Error> for NodeClientError {
    fn from(err: corepc_client::client_sync::Error) -> Self {
        Self::JsonRpc(err)
    }
}

#[cfg(feature = "florestad")]
impl From<electrum_client::Error> for NodeClientError {
    fn from(err: electrum_client::Error) -> Self {
        Self::Electrum(err)
    }
}

/// Errors produced by node configuration, startup, and operations.
#[derive(Debug)]
pub enum NodeError {
    /// Failed to stop a node over JSON-RPC.
    FailedToStop(corepc_client::client_sync::Error),

    /// A JSON-RPC operation failed.
    JsonRpc(corepc_client::client_sync::Error),

    /// Timed out whilst waiting for a peer connection to succeed.
    PeerConnectionTimeout((SocketAddr, SocketAddr)),

    /// A raw CLI argument conflicts with a typed node configuration field.
    ConflictingArgument(String),

    /// Typed node configuration contains an unsupported combination or value.
    InvalidConfiguration(String),

    /// A node does not support a requested command.
    UnsupportedCommand {
        /// Human-readable node name.
        node: &'static str,
        /// Unsupported command name.
        command: &'static str,
    },

    /// A node is unresponsive to client requests.
    UnresponsiveNode {
        /// Human-readable node name.
        node: &'static str,
        /// Client error that made the node unresponsive.
        source: NodeClientError,
    },

    /// Timed out whilst waiting for a node's chain to synchronize.
    ChainSyncTimeout((u32, u32, Duration)), // (target_height, current_height, timeout)

    /// Timed out whilst waiting for a node connection to succeed.
    ConnectionTimeout(Duration),
}

#[rustfmt::skip]
impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err}"),
            Self::JsonRpc(err) => write!(f, "JSON-RPC error: {err}"),
            Self::PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            Self::ConflictingArgument(arg) => write!(f, "Raw node argument conflicts with typed configuration: {arg}"),
            Self::InvalidConfiguration(description) => write!(f, "Invalid node configuration: {description}"),
            Self::UnsupportedCommand { node, command } => write!(f, "`{node}` does not support the `{command}` command"),
            Self::UnresponsiveNode { node, source } => write!(f, "`{node}` is unresponsive: {source}"),
            Self::ChainSyncTimeout((target_height, current_height, timeout)) => write!(
                f,
                "Timed out after {} seconds whilst waiting for the node's chain to synchronize to height={} (current height={})",
                timeout.as_secs(), target_height, current_height
            ),
            Self::ConnectionTimeout(timeout) => write!(
                f,
                "Timed out after {} seconds whilst waiting for a node connection to succeed",
                timeout.as_secs()
            ),
        }
    }
}

impl error::Error for NodeError {}
