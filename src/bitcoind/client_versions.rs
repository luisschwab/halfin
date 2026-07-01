#![allow(unused_imports)]

#[cfg(feature = "bitcoind")]
pub(super) use corepc_client::{client_sync::v30::*, types::v30 as vtype};
