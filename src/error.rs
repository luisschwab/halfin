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
    BothDirsSpecified,

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
            Self::BothDirsSpecified => write!(f, "Both `tmpdir` and `staticdir` were specified. You must choose one or neither"),
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
