// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`BitcoinD`] and [`UtreexoD`].

use std::thread;
use std::time::Duration;
use std::time::Instant;

use halfin::bitcoind::BitcoinD;
use halfin::utreexod::UtreexoD;

fn wait_for_height(node: &UtreexoD, height: u32) {
    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    while start.elapsed() < timeout {
        if node.get_height().unwrap() >= height {
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("timeout waiting for utreexod to reach height {}", height);
}

/// Verify that a [`BitcoinD`] and [`UtreexoD`] node can connect to each other.
#[test]
fn test_bitcoind_utreexod_addnode() {
    let bitcoind = BitcoinD::download_new().unwrap();
    let utreexod = UtreexoD::download_new().unwrap();

    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);
    assert_eq!(utreexod.get_peer_count().unwrap(), 0);

    bitcoind.add_peer(utreexod.get_p2p_socket()).unwrap();

    assert_eq!(bitcoind.get_peer_count().unwrap(), 1);
    assert_eq!(utreexod.get_peer_count().unwrap(), 1);
}

/// Verify that blocks mined on [`BitcoinD`] propagate to a connected [`UtreexoD`].
#[test]
fn test_bitcoind_blocks_propagate_to_utreexod() {
    let bitcoind = BitcoinD::download_new().unwrap();
    let utreexod = UtreexoD::download_new().unwrap();

    // Mine blocks before connecting so utreexod syncs them on connect
    bitcoind.generate(21).unwrap();
    assert_eq!(bitcoind.get_height().unwrap(), 21);

    utreexod.add_peer(bitcoind.get_p2p_socket()).unwrap();

    wait_for_height(&utreexod, 21);
    assert_eq!(utreexod.get_height().unwrap(), 21);
}

// Doesn't work
// BitcoinD needs to mine and only then connect to UtreexoD.
// UtreexoD appears to not sync headers after the initial handshake.
#[test]
#[ignore]
fn test_bitcoind_utreexod_chain_sync() {
    let bitcoind = BitcoinD::download_new().unwrap();
    let utreexod = UtreexoD::download_new().unwrap();

    bitcoind.add_peer(utreexod.get_p2p_socket()).unwrap();

    let bitcoind_peers = bitcoind
        .get_rpc_client()
        .call::<serde_json::Value>("getpeerinfo", &[])
        .unwrap();
    let utreexod_peers = utreexod
        .get_rpc_client()
        .call::<serde_json::Value>("getpeerinfo", &[])
        .unwrap();

    println!("bitcoind peers: {:#?}", bitcoind_peers);
    println!("utreexod peers: {:#?}", utreexod_peers);

    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 10);
    assert_eq!(utreexod.get_height().unwrap(), 10);

    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 20);
    assert_eq!(utreexod.get_height().unwrap(), 20);
}
