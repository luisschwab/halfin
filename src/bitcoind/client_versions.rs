#![allow(unused_imports)]

#[cfg(feature = "bitcoind_31_0")]
pub use corepc_client::{client_sync::v30::*, types::v30 as vtype};
