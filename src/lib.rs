//! # Halfin
//!
//! A bitcoin node running utility for integration testing.
//!
//! ## Supported Implementations and Versions
//!
//! | Implementation | Versions | Feature Flag   |
//! |----------------|----------|--------------- |
//! | [`bitcoind`]   | v30.2    | bitcoind_30_2  |
//! | [`utreexod`]   | v0.5.0   | utreexod_0_5_0 |
//!
//! ## Example
//!
//! ```rust,no_run
//! use halfin::bitcoind::BitcoinD;
//! use halfin::utreexod::UtreexoD;
//!
//! let bitcoind = BitcoinD::download_new().unwrap();
//! bitcoind.generate(10).unwrap();
//! assert_eq!(bitcoind.get_height().unwrap(), 10);
//!
//! let utreexod = UtreexoD::download_new().unwrap();
//! utreexod.generate(10).unwrap();
//! assert_eq!(utreexod.get_height().unwrap(), 10);
//! ```
//!
//! [`bitcoind`]: <https://github.com/bitcoin/bitcoin>
//! [`utreexod`]: <https://github.com/utreexo/utreexod>

use core::fmt;
use core::net::Ipv4Addr;
use core::net::SocketAddr;
use std::net::TcpListener;
use std::path::PathBuf;
use tempfile::TempDir;

pub use bitcoind::BitcoinD;
pub use utreexod::UtreexoD;

pub mod bitcoind;
pub mod utreexod;

/// IPv4 Localhost address.
const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// The maximum number of attempts at instantiating a [`BitcoinD`] or [`UtreexoD`].
pub const MAX_RETRIES_NODE_BUILDING: u8 = 5;

/// Ask the OS for an available port, immediately unbind and return it.
///
/// Inlining is needed to curb TOCTOU race conditions.
#[inline]
pub fn get_available_port() -> u16 {
    TcpListener::bind((LOCALHOST, 0))
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

#[derive(Debug)]
pub enum Error {
    /// The binary was not found at the expected location.
    BinaryNotFound(PathBuf),
    /// Failed to spawn a [process](std::process::Child) for [`BitcoinD`]/[`UtreexoD`].
    FailedToSpawn(std::io::Error),
    /// Failed to instantiate a [`BitcoinD`]/[`UtreexoD`] after [`NODE_BUILDING_MAX_RETRIES`] attempts.
    ExhaustedNodeBuildingRetries,
    /// Failed to stop [`BitcoinD`] or [`UtreexoD`] over JSON-RPC (e.g. `bitcoin-cli -regtest stop`).
    FailedToStop(corepc_client::client_sync::Error),
    /// I/O errors.
    Io(std::io::Error),
    /// JSON-RPC Errors.
    JsonRpc(corepc_client::client_sync::Error),
    /// Timed out whilst waiting for peer connection to succeed.
    PeerConnectionTimeout((SocketAddr, SocketAddr)),
    /// Both `tmpdir` and `workdir` were specified.
    BothDirsSpecified,
    /// [`BitcoinD`] is unresponsive (it's probably not running).
    UnresponsiveBitcoinD(corepc_client::client_sync::Error),
    /// [`UtreexoD`] is unresponsive (it's probably not running).
    UnresponsiveUtreexoD(corepc_client::client_sync::Error),
    /// Timed out whilst waiting for the cookie file to be generated.
    CookieFileTimeout(PathBuf),
    /// Timed out whilst waiting for the JSON-RPC client to be ready.
    RpcClientSetupTimeout,
    /// Received an unexpected response from the JSON-RPC server
    UnexpectedResponse,
}

#[rustfmt::skip]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            BinaryNotFound(path) => write!(f, "The `utreexod` binary was not found at the expected location: {}", path.display()),
            FailedToSpawn(err) => write!(f, "Failed to spawn a process for the node: {err:?}"),
            ExhaustedNodeBuildingRetries => write!(f, "Failed to instantiate the node after {} attempts", NODE_BUILDING_MAX_RETRIES),
            FailedToStop(err) => write!(f, "Failed to stop the node over JSON-RPC: {err:?}"),
            Io(err) => write!(f, "I/O Error: {err:?}"),
            JsonRpc(err) => write!(f, "JSON-RPC Error: {err:?}"),
            PeerConnectionTimeout((local_socket, remote_socket)) => write!(f, "Timed out whilst waiting for connection between local={local_socket} and remote={remote_socket}"),
            BothDirsSpecified => write!(f, "Both `tempdir` and `workdir` were specified. You must choose one and only one"),
            UnresponsiveBitcoinD(err) => write!(f, "`BitcoinD` is unresponsive to JSON-RPC calls: {err:?}"),
            UnresponsiveUtreexoD(err) => write!(f, "`UtreexoD` is unresponsive to JSON-RPC calls: {err:?}"),
            CookieFileTimeout(cookie_path) => write!(f, "Timed out whilst waiting for the cookie={} to be generated", cookie_path.display()),
            RpcClientSetupTimeout => write!(f, "Timed out whilst waiting for the JSON-RPC client to be ready"),
            UnexpectedResponse => write!(f, "Received an unexpected response from the JSON-RPC server"),
        }
    }
}
