// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(all(feature = "bitcoind_31_0", feature = "utreexod_0_5_0"))]

use std::thread::sleep;
use std::time::Duration;

use halfin::Node;
use halfin::bitcoind::BitcoinD;
use halfin::utreexod::UtreexoD;

/// Verify that [`Node::call`] works by calling `uptime`.
#[test]
fn test_node_call() {
    let bitcoind = BitcoinD::new().unwrap();
    sleep(Duration::from_millis(100));
    let utreexod = UtreexoD::new().unwrap();

    sleep(Duration::from_secs(2));

    let bitcoind_uptime = bitcoind.call("uptime", &[]).unwrap();
    let utreexod_uptime = utreexod.call("uptime", &[]).unwrap();

    println!("bitcoind uptime: {}", bitcoind_uptime);
    println!("utreexod uptime: {}", utreexod_uptime);
}
