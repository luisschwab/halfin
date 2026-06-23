// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration test that exercises `tracing` logging for node and indexer
//! processes: halfin's own `debug!` instrumentation as well as each process's
//! `stdout` & `stderr` piped in as `trace!` events.
//!
//! ```sh
//! cargo test --test logging -- --nocapture
//! ```

#![cfg(all(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]

use std::thread;
use std::time::Duration;

use halfin::bitcoind::BitcoinD;
use halfin::connect;
use halfin::electrsd::ElectrsD;
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

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrsd.trigger().unwrap();

    thread::sleep(Duration::from_millis(500));
}
