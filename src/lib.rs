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

use core::error;
use core::fmt;
use core::net::Ipv4Addr;
use corepc_client::client_sync;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;

pub mod bitcoind;
pub mod utreexod;

/// IPv4 Localhost address.
const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

pub fn get_available_port() -> u16 {
    let mut prng = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();

    loop {
        // XOR-shift to get next pseudo-random number.
        prng ^= prng << 13;
        prng ^= prng >> 17;
        prng ^= prng << 5;

        // Pick a outside of the system/ well known range.
        let port = (prng % (65535 - 1024) + 1024) as u16;

        if TcpListener::bind((LOCALHOST, port)).is_ok() {
            return port;
        }
    }
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

/// Halfin errors.
#[derive(Debug)]
pub enum Error {
    /// A standard I/O error (e.g. failed to spawn a process or create a file).
    Io(io::Error),
    /// An error returned by the JSON-RPC client.
    Rpc(client_sync::Error),
    /// A method was called that requires a Cargo feature which is not enabled.
    NoFeature,
    /// A required environment variable is not set.
    NoEnvVar,
    /// The `bitcoind` binary could not be located.
    BitcoinDNotFound,
    /// The `utreexod` binary could not be located.
    UtreexoDNotFound,
    /// The node process exited before it was expected to.
    EarlyExit(ExitStatus),
    /// Both `tmpdir` and `staticdir` were specified in the configuration,
    /// which is not allowed — exactly one must be set.
    BothDirsSpecified,
    /// The deprecated `-rpcuser`/`-rpcpassword` flags were used.
    ///
    /// Use `-rpcauth` instead.
    RpcUserAndPasswordUsed,
    /// `bitcoind` started but is not reachable via RPC.
    BitcoinDNotRunning(String),
    /// `utreexod` started but is not reachable via RPC.
    UtreexoDNotRunning(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            Io(e) => write!(f, "io error: {}", e),
            Rpc(e) => write!(f, "rpc error: {}", e),
            NoFeature => write!(f, "called a method requiring a feature that is not enabled"),
            NoEnvVar => write!(f, "required environment variable is not set"),
            BitcoinDNotFound => write!(
                f,
                "`bitcoind` not found: set `BITCOIND_EXE` or enable the `download` feature"
            ),
            UtreexoDNotFound => write!(
                f,
                "`utreexod` not found: set `UTREEXOD_EXE` or enable the `download` feature"
            ),
            EarlyExit(status) => write!(f, "process terminated early with exit code {}", status),
            BothDirsSpecified => write!(f, "`tmpdir` and `staticdir` cannot both be specified"),
            RpcUserAndPasswordUsed => write!(
                f,
                "`-rpcuser`/`-rpcpassword` are deprecated, use `-rpcauth` instead"
            ),
            BitcoinDNotRunning(msg) => write!(f, "bitcoind is not reachable: {}", msg),
            UtreexoDNotRunning(msg) => write!(f, "utreexod is not reachable: {}", msg),
        }
    }
}

impl error::Error for Error {
    /// Returns the wrapped lower-level error for [`HalfinError::Io`] and
    /// [`HalfinError::Rpc`]; `None` for all other variants.
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        use Error::*;
        match self {
            Io(e) => Some(e),
            Rpc(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<client_sync::Error> for Error {
    fn from(e: client_sync::Error) -> Self {
        Error::Rpc(e)
    }
}
