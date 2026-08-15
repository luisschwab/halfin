//! # Error
//!
//! Error types returned by node and indexer process management helpers.

use core::error;
use core::fmt;
use std::io;
use std::path::PathBuf;

#[cfg(any(feature = "electrs", feature = "electrumx"))]
use crate::indexer::IndexerError;
#[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
use crate::node::NodeError;

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

    /// Failed to spawn a process for a node or indexer.
    FailedToSpawn(io::Error),

    /// Failed to start a node or indexer after the configured number of attempts.
    StartupAttemptsExhausted(u8),

    /// I/O errors.
    Io(io::Error),

    /// Both `tmpdir` and `staticdir` were specified.
    BothDirsSpecified,

    /// Timed out whilst waiting for a node or indexer client to be ready.
    ClientSetupTimeout,

    /// Received an unexpected response from a node or indexer.
    UnexpectedResponse(String),

    /// A node operation failed.
    #[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
    Node(NodeError),

    /// An indexer operation failed.
    #[cfg(any(feature = "electrs", feature = "electrumx"))]
    Indexer(IndexerError),
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryPathNotAbsolute { bin_name, path } => write!(f, "The `{}` binary path is not absolute (path={})", bin_name, path),
            Self::BinaryPathNotFile { bin_name, path } => write!(f, "The `{}` binary path is not a file (path={})", bin_name, path),
            Self::BinaryNotFound((bin_name, path)) => write!(f, "The `{}` binary was not found at the expected location={}", bin_name, path.display()),
            Self::FailedToSpawn(err) => write!(f, "Failed to spawn a process: {err}"),
            Self::StartupAttemptsExhausted(attempts) => write!(f, "Failed to start the process after {attempts} attempts"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::BothDirsSpecified => write!(f, "Both `tmpdir` and `staticdir` were specified. You must choose one or neither"),
            Self::ClientSetupTimeout => write!(f, "Timed out whilst waiting for the client to be ready"),
            Self::UnexpectedResponse(err) => write!(f, "Received an unexpected response from a node or indexer: {err}"),
            #[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
            Self::Node(err) => fmt::Display::fmt(err, f),
            #[cfg(any(feature = "electrs", feature = "electrumx"))]
            Self::Indexer(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl error::Error for Error {}

#[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
impl From<NodeError> for Error {
    fn from(err: NodeError) -> Self {
        Self::Node(err)
    }
}

#[cfg(any(feature = "electrs", feature = "electrumx"))]
impl From<IndexerError> for Error {
    fn from(err: IndexerError) -> Self {
        Self::Indexer(err)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
    use super::Error;
    #[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
    use crate::node::NodeError;

    #[test]
    #[cfg(any(feature = "bitcoind", feature = "florestad", feature = "utreexod"))]
    fn node_error_conversion_preserves_display() {
        let err = Error::from(NodeError::JsonRpc(
            corepc_client::client_sync::Error::MissingUserPassword,
        ));

        assert_eq!(
            err.to_string(),
            "JSON-RPC error: missing user and/or password"
        );
    }
}
