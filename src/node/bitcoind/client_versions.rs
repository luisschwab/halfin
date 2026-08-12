// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(unused_imports)]

#[cfg(feature = "bitcoind")]
pub(super) use corepc_client::client_sync::v30::*;
#[cfg(feature = "bitcoind")]
pub(super) use corepc_client::types::v30 as vtype;
