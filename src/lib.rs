//! # halfin
//!
//! Start local Bitcoin [`Node`] and Electrum [`Indexer`] implementations for integration tests.
//!
//! The crate finds each enabled program and starts it in an isolated data directory.
//! It assigns local ports and supplies typed clients for test operations.
//! It also stops each child process when Rust drops its handle.
//!
//! ## Supported implementations
//!
//! | Kind    | Implementation | Version   | Feature Flag | Default Feature | Notes             |
//! |---------|----------------|-----------|--------------|-----------------|-------------------|
//! | [`Node`] | `bitcoind` | `v31.0` | `bitcoind` | Yes | |
//! | [`Node`] | `utreexod` | `v0.6.0` | `utreexod` | Yes | |
//! | [`Node`] | `florestad` | `v0.9.1` | `florestad` | No | |
//! |         |                |           |              |                 |                   |
//! | [`Indexer`] | `electrs` | `v0.11.1` | `electrs` | No | |
//! | [`Indexer`] | `electrumx` | `v1.20.0` | `electrumx` | No | Needs Python 3.10 |
//!
//! ## Start and connect two [`Node`] implementations
//!
//! ```rust,ignore
//! use halfin::node::bitcoind::BitcoinD;
//! use halfin::node::connect;
//! use halfin::node::utreexod::UtreexoD;
//!
//! let bitcoind = BitcoinD::new().unwrap();
//! bitcoind.generate(10).unwrap();
//! assert_eq!(bitcoind.get_chain_tip().unwrap(), 10);
//!
//! let utreexod = UtreexoD::new().unwrap();
//! utreexod.generate(10).unwrap();
//! assert_eq!(utreexod.get_chain_tip().unwrap(), 10);
//!
//! connect(&bitcoind, &utreexod).unwrap();
//! ```
//!
//! [`bitcoind`]: <https://github.com/bitcoin/bitcoin>
//! [`florestad`]: <https://github.com/getfloresta/Floresta>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>
//! [`electrs`]: <https://github.com/romanz/electrs>
//! [`electrumx`]: <https://github.com/spesmilo/electrumx>
//! [`Indexer`]: crate::indexer::Indexer
//! [`Node`]: crate::node::Node

use core::net::Ipv4Addr;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::env;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::fs;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::io::BufRead;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::io::BufReader;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::io::Read;
use std::net::TcpListener;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

pub use corepc_client::bitcoin;
pub use serde_json;
use tempfile::TempDir;
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use tracing::info;

pub use crate::error::Error;

pub mod error;
#[cfg(any(feature = "electrs", feature = "electrumx"))]
pub mod indexer;
pub mod node;

/// IPv4 localhost address.
const IPV4_LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// Maximum number of process start attempts.
pub const SPAWN_ATTEMPTS: u8 = 5;

/// Interval between process start attempts.
pub const SPAWN_INTERVAL: Duration = Duration::from_millis(500);

/// Period between polls for [`connect`](crate::node::connect) and
/// [`wait_for_height`](crate::node::wait_for_height).
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Timeout for [`connect`](crate::node::connect) and
/// [`wait_for_height`](crate::node::wait_for_height).
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between [`Node`](crate::node::Node) connection attempts.
pub const CONNECTION_INTERVAL: Duration = Duration::from_millis(150);

/// Timeout for [`Node`](crate::node::Node) connection.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask the operating system for an available port, release the port, and return it.
///
/// # Panics
///
/// Panics if the operating system cannot bind a temporary localhost port or report its socket
/// address.
#[inline]
pub fn get_available_port() -> u16 {
    TcpListener::bind((IPV4_LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Find the first raw argument owned by typed or dynamic configuration.
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) fn find_conflicting_argument<S: AsRef<str>>(
    args: &[S],
    option_names: &[&str],
    boolean_option_names: &[&str],
) -> Option<String> {
    args.iter().find_map(|arg| {
        let arg = arg.as_ref();
        let option = arg.strip_prefix('-')?.trim_start_matches('-');
        let name = option
            .split_once('=')
            .map_or(option, |(name, _)| name)
            .to_ascii_lowercase();

        let normalized_boolean = name.strip_prefix("no-").or_else(|| name.strip_prefix("no"));
        let is_attached_short_option = !arg.starts_with("--")
            && option_names
                .iter()
                .any(|option_name| option_name.len() == 1 && name.starts_with(option_name));
        let is_conflict = option_names.contains(&name.as_str())
            || is_attached_short_option
            || normalized_boolean.is_some_and(|name| boolean_option_names.contains(&name));

        is_conflict.then(|| arg.to_string())
    })
}

/// Start a background thread that reads each line from `reader`.
/// The thread emits each line as an [`info!`] event with the `source` prefix.
///
/// Use this function to send child process output to [`tracing`].
/// The thread stops at the end of the input stream.
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) fn pipe_to_tracing<R: Read + Send + 'static>(reader: R, source: &'static str) {
    std::thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            // Skip blank lines so the log stream mirrors the output.
            if !line.trim().is_empty() {
                info!("{source}: {line}");
            }
        }
    });
}

/// Stores a temporary or persistent process data directory.
///
/// * [`DataDir::Temporary`] contains a [`TempDir`]. Rust deletes the directory when it drops this
///   value.
/// * [`DataDir::Persistent`] contains a [`PathBuf`]. Rust keeps this directory after `Drop`.
#[derive(Debug)]
pub enum DataDir {
    /// A persistent directory that remains after `Drop`.
    Persistent(PathBuf),
    /// A temporary directory that Rust deletes at `Drop`.
    Temporary(TempDir),
}

impl DataDir {
    /// Return the file system path for either variant.
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Persistent(path) => path.to_owned(),
            Self::Temporary(tmp_dir) => tmp_dir.path().to_path_buf(),
        }
    }
}

/// Resolve and create a [`Node`](crate::node::Node) or [`Indexer`](crate::indexer::Indexer) data
/// directory.
#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) fn init_data_dir(
    tmpdir: Option<&Path>,
    staticdir: Option<&Path>,
    prefix: &str,
) -> Result<DataDir, Error> {
    if tmpdir.is_some() && staticdir.is_some() {
        return Err(Error::BothDirsSpecified);
    }

    if let Some(staticdir) = staticdir {
        fs::create_dir_all(staticdir).map_err(Error::Io)?;
        return Ok(DataDir::Persistent(staticdir.to_path_buf()));
    }

    let tmpdir = tmpdir
        .map(Path::to_path_buf)
        .or_else(|| env::var("TEMPDIR_ROOT").map(PathBuf::from).ok());
    match tmpdir {
        Some(tmpdir) => tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(tmpdir)
            .map(DataDir::Temporary)
            .map_err(Error::Io),
        None => tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .map(DataDir::Temporary)
            .map_err(Error::Io),
    }
}

#[cfg(all(
    test,
    any(
        feature = "bitcoind",
        feature = "florestad",
        feature = "utreexod",
        feature = "electrs",
        feature = "electrumx"
    )
))]
mod tests {
    use super::*;

    #[test]
    fn data_directory_configuration_is_shared() {
        let root = tempfile::tempdir().unwrap();
        let staticdir = root.path().join("static");

        assert!(matches!(
            init_data_dir(Some(root.path()), Some(&staticdir), "halfin-test-"),
            Err(Error::BothDirsSpecified)
        ));

        let data_dir = init_data_dir(None, Some(&staticdir), "halfin-test-").unwrap();
        assert_eq!(data_dir.path(), staticdir);
        assert!(matches!(data_dir, DataDir::Persistent(_)));
    }
}
