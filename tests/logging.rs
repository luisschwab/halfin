// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration test for [`Node`] and [`Indexer`] log output.
//!
//! This test writes halfin `debug!` events to the test output.
//! It also writes child process `stdout` and `stderr` as `trace!` events.
//! Use this command to show the events:
//!
//! ```sh
//! cargo test --test logging -- --nocapture
//! ```
//!
//! [`Indexer`]: halfin::indexer::Indexer
//! [`Node`]: halfin::node::Node

#![cfg(all(feature = "bitcoind", feature = "utreexod", feature = "electrs"))]

use std::thread;
use std::time::Duration;

use halfin::indexer::electrsd::ElectrsD;
use halfin::node::bitcoind::BitcoinD;
use halfin::node::connect;
use halfin::node::utreexod::UtreexoD;
use tracing::Level;

#[test]
/// Use `cargo test --test logging -- --nocapture` to show the tracing events.
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
