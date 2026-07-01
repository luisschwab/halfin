//! # halfin
//!
//! A bitcoin node and indexer running utility for integration testing.
//!
//! > A {regtest} bitcoin node runner 🏃‍♂️
//!
//! This crate makes it simple to run regtest [`bitcoind`], [`utreexod`],
//! and [`electrs`] instances from Rust code, useful in integration test contexts.
//!
//! ## Supported Implementations
//!
//! | Kind    | Implementation | Version   | Feature Flag | Default Feature |
//! |---------|----------------|-----------|--------------|-----------------|
//! | Node    | `bitcoind`     | `v31.0`   | `bitcoind`   | Yes             |
//! | Node    | `utreexod`     | `v0.6.0`  | `utreexod`   | Yes             |
//! |         |                |                          |                 |
//! | Indexer | `electrs`      | `v0.11.1` | `electrs`    | No              |
//!
//! ## Example
//!
//! ```rust,ignore
//! use halfin::bitcoind::BitcoinD;
//! use halfin::connect;
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
//! [`electrs`]: <https://github.com/romanz/electrs>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>

use core::net::Ipv4Addr;

#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

pub use serde_json;
use tempfile::TempDir;
#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
use tracing::trace;

#[allow(unused)]
#[cfg(feature = "bitcoind")]
pub(crate) use bitcoind::BitcoinD;
#[allow(unused)]
#[cfg(feature = "electrs")]
pub(crate) use electrsd::ElectrsD;
#[allow(unused)]
#[cfg(feature = "utreexod")]
pub(crate) use utreexod::UtreexoD;

pub use crate::error::Error;

#[cfg(feature = "bitcoind")]
pub mod bitcoind;
#[cfg(feature = "electrs")]
pub mod electrsd;
pub mod error;
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

/// Spawn a background thread that reads `reader` line by line and re-emits
/// each line as a [`trace!`] event, prefixed with `source`.
///
/// Used to pipe a child [`BitcoinD`]/[`UtreexoD`]/[`ElectrsD`] process `stdout`/`stderr`
/// into [`tracing`]. The thread exits on EOF, which happens when the process
/// dies and its pipe is closed.
#[cfg(any(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]
pub(crate) fn pipe_to_tracing<R: Read + Send + 'static>(reader: R, source: &'static str) {
    std::thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            // Skip blank lines so the trace stream mirrors the node's output.
            if !line.trim().is_empty() {
                trace!("{source}: {line}");
            }
        }
    });
}

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
