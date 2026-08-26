//! Error types for [`Node`] configuration, startup, and operation.
//!
//! [`NodeError`] identifies the failed [`Node`] operation.
//! [`NodeClientError`] contains the error from the selected client protocol.
//!
//! [`Node`]: crate::node::Node

use core::error;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;

/// Client errors that can make a [`Node`](crate::node::Node) unresponsive.
#[derive(Debug)]
#[non_exhaustive]
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

/// Errors produced by [`Node`](crate::node::Node) configuration, startup, and operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum NodeError {
    /// A JSON-RPC request did not stop a [`Node`](crate::node::Node).
    FailedToStop(corepc_client::client_sync::Error),

    /// A JSON-RPC operation failed.
    JsonRpc(corepc_client::client_sync::Error),

    /// A peer connection did not complete before the timeout.
    PeerConnectionTimeout((SocketAddr, SocketAddr)),

    /// A raw CLI argument conflicts with a typed [`Node`](crate::node::Node) configuration field.
    ConflictingArgument(String),

    /// Typed [`Node`](crate::node::Node) configuration contains an unsupported combination or
    /// value.
    InvalidConfiguration(String),

    /// A [`Node`](crate::node::Node) does not support a requested command.
    UnsupportedCommand {
        /// Human-readable [`Node`](crate::node::Node) name.
        node: &'static str,
        /// Unsupported command name.
        command: &'static str,
    },

    /// A [`Node`](crate::node::Node) is unresponsive to client requests.
    UnresponsiveNode {
        /// Human-readable [`Node`](crate::node::Node) name.
        node: &'static str,
        /// Client error that made the [`Node`](crate::node::Node) unresponsive.
        source: NodeClientError,
    },

    /// A [`Node`](crate::node::Node) did not synchronize its chain before the timeout.
    ChainSyncTimeout((u32, u32, Duration)), // (target_height, current_height, timeout)

    /// A [`Node`](crate::node::Node) connection did not complete before the timeout.
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

#[cfg(test)]
mod tests {
    use core::net::SocketAddr;
    use core::time::Duration;

    use corepc_client::client_sync::Error as RpcError;

    use super::NodeClientError;
    use super::NodeError;

    /// Exercise JSON-RPC client error conversion and display.
    #[test]
    fn json_rpc_client_errors_convert_and_display() {
        let error = NodeClientError::from(RpcError::InvalidCookieFile);
        assert!(matches!(error, NodeClientError::JsonRpc(_)));
        drop(error.to_string());
    }

    /// Exercise Electrum client error conversion and display.
    #[cfg(feature = "florestad")]
    #[test]
    fn electrum_client_errors_convert_and_display() {
        let error =
            NodeClientError::from(electrum_client::Error::Message("unavailable".to_string()));
        assert!(matches!(error, NodeClientError::Electrum(_)));
        drop(error.to_string());
    }

    /// Exercise every node error display branch.
    #[test]
    fn node_errors_are_displayable() {
        let local_socket = SocketAddr::from(([127, 0, 0, 1], 18_444));
        let remote_socket = SocketAddr::from(([127, 0, 0, 1], 18_445));
        let timeout = Duration::from_secs(1);
        let errors = [
            NodeError::FailedToStop(RpcError::InvalidCookieFile),
            NodeError::JsonRpc(RpcError::InvalidCookieFile),
            NodeError::PeerConnectionTimeout((local_socket, remote_socket)),
            NodeError::ConflictingArgument("rpcport".to_string()),
            NodeError::InvalidConfiguration("invalid value".to_string()),
            NodeError::UnsupportedCommand {
                node: "FakeNode",
                command: "generate",
            },
            NodeError::UnresponsiveNode {
                node: "FakeNode",
                source: NodeClientError::JsonRpc(RpcError::InvalidCookieFile),
            },
            NodeError::ChainSyncTimeout((10, 9, timeout)),
            NodeError::ConnectionTimeout(timeout),
        ];

        for error in errors {
            drop(error.to_string());
        }
    }
}
