// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(unused_imports)]

#[cfg(feature = "bitcoind")]
pub(super) use corepc_regtest_client::types::v30 as vtype;
#[cfg(feature = "bitcoind")]
pub(super) use corepc_regtest_client::v30::*;
