//! Error types for [`Indexer`] configuration, startup, and operation.
//!
//! [`IndexerError`] identifies configuration errors, client errors, and indexing timeouts.
//!
//! [`Indexer`]: crate::indexer::Indexer

use core::error;
use core::fmt;
use core::time::Duration;

/// Errors produced by [`Indexer`](crate::indexer::Indexer) configuration, startup, and operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum IndexerError {
    /// A raw CLI argument conflicts with typed or dynamic [`Indexer`](crate::indexer::Indexer)
    /// configuration.
    ConflictingArgument(String),

    /// [`Indexer`](crate::indexer::Indexer) configuration contains an unsupported combination or
    /// value.
    InvalidConfiguration(String),

    /// The required Python interpreter is not available or cannot run.
    #[cfg(feature = "electrumx")]
    InvalidPython(String),

    /// An external [`Indexer`](crate::indexer::Indexer) does not support the
    /// [`Node`](crate::node::Node) implementation.
    UnsupportedBackend {
        /// Human-readable [`Node`](crate::node::Node) name.
        node: &'static str,
    },

    /// An [`Indexer`](crate::indexer::Indexer) is unresponsive to Electrum requests.
    UnresponsiveIndexer {
        /// Human-readable [`Indexer`](crate::indexer::Indexer) name.
        indexer: &'static str,
        /// Electrum client error that made the [`Indexer`](crate::indexer::Indexer) unresponsive.
        source: electrum_client::Error,
    },

    /// An [`Indexer`](crate::indexer::Indexer) did not index the expected data before the timeout.
    IndexingTimeout {
        /// Human-readable [`Indexer`](crate::indexer::Indexer) name.
        indexer: &'static str,
        /// Description of the expected indexed data.
        description: String,
        /// Maximum wait time.
        timeout: Duration,
    },
}

#[rustfmt::skip]
impl fmt::Display for IndexerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingArgument(arg) => write!(f, "Raw indexer argument conflicts with typed or dynamic configuration: {arg}"),
            Self::InvalidConfiguration(description) => write!(f, "Invalid indexer configuration: {description}"),
            #[cfg(feature = "electrumx")]
            Self::InvalidPython(description) => write!(f, "Invalid Python runtime for `ElectrumX`: {description}"),
            Self::UnsupportedBackend { node } => write!(f, "`{node}` cannot be used as a backing node for an indexer"),
            Self::UnresponsiveIndexer { indexer, source } => write!(f, "`{indexer}` is unresponsive to Electrum requests: {source}"),
            Self::IndexingTimeout { indexer, description, timeout } => write!(f, "Timed out after {} seconds whilst waiting for `{indexer}` to index {description}", timeout.as_secs()),
        }
    }
}

impl error::Error for IndexerError {}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::IndexerError;

    /// Exercise every enabled indexer error display branch.
    #[test]
    fn indexer_errors_are_displayable() {
        let errors = vec![
            IndexerError::ConflictingArgument("db-dir".to_string()),
            IndexerError::InvalidConfiguration("invalid value".to_string()),
            IndexerError::UnsupportedBackend { node: "FakeNode" },
            IndexerError::UnresponsiveIndexer {
                indexer: "TestIndexer",
                source: electrum_client::Error::Message("unavailable".to_string()),
            },
            IndexerError::IndexingTimeout {
                indexer: "TestIndexer",
                description: "block 42".to_string(),
                timeout: Duration::from_secs(10),
            },
            #[cfg(feature = "electrumx")]
            IndexerError::InvalidPython("unavailable".to_string()),
        ];

        for error in errors {
            drop(error.to_string());
        }
    }
}
