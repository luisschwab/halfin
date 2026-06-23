// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration test that exercises the `tracing` logging for both [`BitcoinD`]
//! and [`UtreexoD`]: halfin's own `debug!` instrumentation as well as each
//! node's stdout/stderr piped in as `trace!` events.
//!
//! ```sh
//! cargo test --test logging -- --nocapture
//! ```

#![cfg(all(feature = "bitcoind", feature = "utreexod"))]

use std::thread;
use std::time::Duration;

use halfin::bitcoind::BitcoinD;
use halfin::connect;
use halfin::utreexod::UtreexoD;
use tracing::Level;

#[test]
/// Run with `cargo test --test logging -- --nocapture` to see the tracing events.
fn test_logging_all() {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_test_writer()
        .init();

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(2).unwrap();
    bitcoind.get_block_hash(1).unwrap();
    bitcoind.get_chain_tip().unwrap();

    let utreexod = UtreexoD::new().unwrap();
    utreexod.generate(2).unwrap();

    connect(&bitcoind, &utreexod).unwrap();

    thread::sleep(Duration::from_millis(500));
}
