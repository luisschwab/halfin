// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(all(feature = "bitcoind_31_0", feature = "utreexod_0_5_0"))]

use halfin::Node;
use halfin::bitcoind::BitcoinD;
use halfin::connect;
use halfin::utreexod::UtreexoD;

/// Verify that [`Node::call`] works by calling `uptime`.
#[test]
fn test_node_call() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    let bitcoind_uptime = bitcoind.call("uptime", &[]).unwrap();
    let utreexod_uptime = utreexod.call("uptime", &[]).unwrap();

    println!("BitcoinD uptime: {}", bitcoind_uptime);
    println!("UtreexoD uptime: {}", utreexod_uptime);
}

/// Verify that [`connect`] connects any combination of [`Node`] implementations.
#[test]
fn test_connect() {
    let bitcoind_1 = BitcoinD::new().unwrap();
    let bitcoind_2 = BitcoinD::new().unwrap();
    let utreexod_1 = UtreexoD::new().unwrap();
    let utreexod_2 = UtreexoD::new().unwrap();

    // .. > bitcoind_1 > bitcoind_2 > utreexod_1 > utreexod_2 > ..
    connect(&bitcoind_1, &bitcoind_2).unwrap();
    connect(&bitcoind_2, &utreexod_1).unwrap();
    connect(&utreexod_1, &utreexod_2).unwrap();
    connect(&utreexod_2, &bitcoind_1).unwrap();
}
