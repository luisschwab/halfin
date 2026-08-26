//! Error types for process management and backend operations.
//!
//! [`Error`] contains errors that are common to [`Node`] and [`Indexer`] implementations.
//! It also contains feature-gated [`Node`] and [`Indexer`] errors.
//!
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

use core::error;
use core::fmt;
use std::io;
use std::path::PathBuf;

#[cfg(halfin_indexer)]
use crate::indexer::IndexerError;
#[cfg(halfin_node)]
use crate::node::NodeError;

/// Errors returned by [`Node`](crate::node::Node) and [`Indexer`](crate::indexer::Indexer) process
/// management helpers.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The binary path is not absolute.
    BinaryPathNotAbsolute {
        /// Name of the binary for the rejected path.
        bin_name: String,
        /// Rejected file system path.
        path: String,
    },

    /// The binary path is not a file.
    BinaryPathNotFile {
        /// Name of the binary for the rejected path.
        bin_name: String,
        /// Rejected file system path.
        path: String,
    },

    /// The binary was not found at the expected location.
    BinaryNotFound((String, PathBuf)),

    /// The system could not start a [`Node`](crate::node::Node) or
    /// [`Indexer`](crate::indexer::Indexer) process.
    FailedToSpawn(io::Error),

    /// The process did not start after the configured number of attempts.
    StartupAttemptsExhausted(u8),

    /// An input or output operation failed.
    Io(io::Error),

    /// The configuration contains both `tmpdir` and `staticdir`.
    BothDirectoriesSpecified,

    /// A [`Node`](crate::node::Node) or [`Indexer`](crate::indexer::Indexer) client did not become
    /// ready before the timeout.
    ClientSetupTimeout,

    /// A [`Node`](crate::node::Node) or [`Indexer`](crate::indexer::Indexer) returned an unexpected
    /// response.
    UnexpectedResponse(String),

    /// A [`Node`](crate::node::Node) operation failed.
    #[cfg(halfin_node)]
    Node(NodeError),

    /// An [`Indexer`](crate::indexer::Indexer) operation failed.
    #[cfg(halfin_indexer)]
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
            Self::BothDirectoriesSpecified => write!(f, "Both `tmpdir` and `staticdir` were specified. You must choose one or neither"),
            Self::ClientSetupTimeout => write!(f, "Timed out whilst waiting for the client to be ready"),
            Self::UnexpectedResponse(err) => write!(f, "Received an unexpected response from a node or indexer: {err}"),
            #[cfg(halfin_node)]
            Self::Node(err) => fmt::Display::fmt(err, f),
            #[cfg(halfin_indexer)]
            Self::Indexer(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl error::Error for Error {}

#[cfg(halfin_node)]
impl From<NodeError> for Error {
    fn from(err: NodeError) -> Self {
        Self::Node(err)
    }
}

#[cfg(halfin_indexer)]
impl From<IndexerError> for Error {
    fn from(err: IndexerError) -> Self {
        Self::Indexer(err)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use super::Error;

    /// Exercise every common error display branch.
    #[test]
    fn common_errors_are_displayable() {
        let errors = [
            Error::BinaryPathNotAbsolute {
                bin_name: "daemon".to_string(),
                path: "daemon".to_string(),
            },
            Error::BinaryPathNotFile {
                bin_name: "daemon".to_string(),
                path: "/missing/daemon".to_string(),
            },
            Error::BinaryNotFound(("daemon".to_string(), PathBuf::from("/missing/daemon"))),
            Error::FailedToSpawn(io::Error::other("spawn failed")),
            Error::StartupAttemptsExhausted(3),
            Error::Io(io::Error::other("I/O failed")),
            Error::BothDirectoriesSpecified,
            Error::ClientSetupTimeout,
            Error::UnexpectedResponse("invalid response".to_string()),
        ];

        for error in errors {
            drop(error.to_string());
        }
    }

    /// Exercise the node error wrapper and conversion.
    #[cfg(halfin_node)]
    #[test]
    fn node_errors_convert_to_common_errors() {
        use crate::node::NodeError;

        let error = Error::from(NodeError::ConflictingArgument("rpcport".to_string()));
        assert!(matches!(
            error,
            Error::Node(NodeError::ConflictingArgument(_))
        ));
        drop(error.to_string());
    }

    /// Exercise the indexer error wrapper and conversion.
    #[cfg(halfin_indexer)]
    #[test]
    fn indexer_errors_convert_to_common_errors() {
        use crate::indexer::IndexerError;

        let error = Error::from(IndexerError::ConflictingArgument("db-dir".to_string()));
        assert!(matches!(
            error,
            Error::Indexer(IndexerError::ConflictingArgument(_))
        ));
        drop(error.to_string());
    }
}
