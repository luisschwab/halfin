//! Errors produced by indexer operations.

use core::error;
use core::fmt;
use core::time::Duration;

/// Errors produced by indexer configuration, startup, and operations.
#[derive(Debug)]
pub enum IndexerError {
    /// A raw CLI argument conflicts with typed or dynamic indexer configuration.
    ConflictingArgument(String),

    /// Indexer configuration contains an unsupported combination or value.
    InvalidConfiguration(String),

    /// The Python interpreter required by the bundled `ElectrumX` launcher is unavailable or
    /// cannot run.
    #[cfg(feature = "electrumx")]
    InvalidPython(String),

    /// The node implementation cannot be used as an external indexer's backend.
    UnsupportedBackend {
        /// Human-readable node name.
        node: &'static str,
    },

    /// An indexer is unresponsive to Electrum requests.
    UnresponsiveIndexer {
        /// Human-readable indexer name.
        indexer: &'static str,
        /// Electrum client error that made the indexer unresponsive.
        source: electrum_client::Error,
    },

    /// Timed out whilst waiting for an indexer to index expected data.
    IndexingTimeout {
        /// Human-readable indexer name.
        indexer: &'static str,
        /// Description of the data that should have been indexed.
        description: String,
        /// Maximum amount of time spent waiting.
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
    use crate::Error;

    #[test]
    fn indexing_timeout_retains_context() {
        let err = Error::from(IndexerError::IndexingTimeout {
            indexer: "ElectrsD",
            description: "block 42".to_string(),
            timeout: Duration::from_secs(10),
        });

        assert_eq!(
            err.to_string(),
            "Timed out after 10 seconds whilst waiting for `ElectrsD` to index block 42"
        );
    }
}
