//! # halfin
//!
//! A bitcoin node and indexer running utility for integration testing.
//!
//! > A runner for bitcoin nodes and indexers 🏃‍♂️
//!
//! This crate makes it simple to run [`bitcoind`], [`utreexod`], [`electrs`],
//! and [`electrumx`] instances from Rust code, useful in integration test
//! contexts.
//!
//! ## Supported Implementations
//!
//! | Kind    | Implementation | Version   | Feature Flag | Default Feature | Notes             |
//! |---------|----------------|-----------|--------------|-----------------|-------------------|
//! | Node    | `bitcoind`     | `v31.0`   | `bitcoind`   | Yes             |                   |
//! | Node    | `utreexod`     | `v0.6.0`  | `utreexod`   | Yes             |                   |
//! |         |                |           |              |                 |                   |
//! | Indexer | `electrs`      | `v0.11.1` | `electrs`    | No              |                   |
//! | Indexer | `electrumx`    | `v1.20.0` | `electrumx`  | No              | Needs Python 3.10 |
//!
//! ## Example
//!
//! ```rust,ignore
//! use halfin::bitcoind::BitcoinD;
//! use halfin::node::connect;
//! use halfin::utreexod::UtreexoD;
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
//! [`utreexod`]: <https://github.com/utreexo/utreexod>
//! [`electrs`]: <https://github.com/romanz/electrs>
//! [`electrumx`]: <https://github.com/spesmilo/electrumx>

use core::net::Ipv4Addr;

#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::env;
#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::fs;
#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
#[cfg(any(
    feature = "bitcoind",
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
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
use tracing::info;

#[allow(unused)]
#[cfg(feature = "bitcoind")]
pub(crate) use bitcoind::BitcoinD;
#[allow(unused)]
#[cfg(feature = "electrs")]
pub(crate) use electrsd::ElectrsD;
#[allow(unused)]
#[cfg(feature = "electrumx")]
pub(crate) use electrumxd::ElectrumxD;
#[allow(unused)]
#[cfg(feature = "utreexod")]
pub(crate) use utreexod::UtreexoD;

pub use crate::error::Error;

#[cfg(feature = "bitcoind")]
pub mod bitcoind;
#[cfg(feature = "electrs")]
pub mod electrsd;
#[cfg(feature = "electrumx")]
pub mod electrumxd;
pub mod error;
#[cfg(any(feature = "electrs", feature = "electrumx"))]
pub mod indexer;
pub mod node;
#[cfg(feature = "utreexod")]
pub mod utreexod;

/// IPv4 localhost address.
const IPV4_LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// Maximum number of attempts at spawning a process.
pub const SPAWN_ATTEMPTS: u8 = 5;

/// Period between attempts at spawning a process.
pub const SPAWN_INTERVAL: Duration = Duration::from_millis(500);

/// Period between polls for [`connect`](crate::node::connect) and [`wait_for_height`](crate::node::wait_for_height).
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Timeout for [`connect`](crate::node::connect) and [`wait_for_height`](crate::node::wait_for_height).
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Period between successive attempts of [`Node`](crate::node::Node) connection.
pub const CONNECTION_INTERVAL: Duration = Duration::from_millis(150);

/// Timeout for [`Node`](crate::node::Node) connection.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask the OS for an available port, immediately unbind and return it.
///
/// # Panics
///
/// Panics if the OS cannot bind a localhost ephemeral port or report the local socket address.
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
        let is_conflict = option_names.contains(&name.as_str())
            || normalized_boolean.is_some_and(|name| boolean_option_names.contains(&name));

        is_conflict.then(|| arg.to_string())
    })
}

/// Spawn a background thread that reads `reader` line by line and re-emits
/// each line as an [`info!`] event, prefixed with `source`.
///
/// Used to pipe a child process' `stdout`/`stderr`
/// into [`tracing`]. The thread exits on EOF, which happens when the process
/// dies and its pipe is closed.
#[cfg(any(
    feature = "bitcoind",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
pub(crate) fn pipe_to_tracing<R: Read + Send + 'static>(reader: R, source: &'static str) {
    std::thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            // Skip blank lines so the log stream mirrors the node's output.
            if !line.trim().is_empty() {
                info!("{source}: {line}");
            }
        }
    });
}

/// Owns a node's working directory, either as a temporary or a persistent path.
///
/// * [`DataDir::Temporary`]: backed by a [`TempDir`]; the directory is
///   deleted automatically when this value is dropped.
/// * [`DataDir::Persistent`]: backed by a plain [`PathBuf`]; the directory
///   survives the process and is never cleaned up automatically.
#[derive(Debug)]
pub enum DataDir {
    /// A persistent directory that is **not** cleaned up on drop.
    Persistent(PathBuf),
    /// A temporary directory that is deleted when this value is dropped.
    Temporary(TempDir),
}

impl DataDir {
    /// Return the underlying filesystem path regardless of variant.
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Persistent(path) => path.to_owned(),
            Self::Temporary(tmp_dir) => tmp_dir.path().to_path_buf(),
        }
    }
}

/// Resolve and create a daemon or indexer data directory.
#[cfg(any(
    feature = "bitcoind",
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
