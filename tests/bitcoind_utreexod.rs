// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`BitcoinD`] and [`UtreexoD`].

#![cfg(all(feature = "bitcoind_31_0", feature = "utreexod_0_5_1"))]

//! Integration tests between [`BitcoinD`] and [`UtreexoD`].

use halfin::bitcoind::BitcoinD;
use halfin::connect;
use halfin::utreexod::UtreexoD;
use halfin::wait_for_height;

/// Verify that [`BitcoinD`] and [`UtreexoD`] can connect to each other.
#[test]
fn test_bitcoind_utreexod_addnode() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);
    assert_eq!(utreexod.get_peer_count().unwrap(), 0);

    connect(&bitcoind, &utreexod).unwrap();

    assert_eq!(bitcoind.get_peer_count().unwrap(), 1);
    assert_eq!(utreexod.get_peer_count().unwrap(), 1);
}

/// Verify that blocks mined on [`BitcoinD`] propagate to a connected [`UtreexoD`].
#[test]
fn test_bitcoind_blocks_propagate_to_utreexod() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    // Mine blocks before connecting so utreexod syncs them on connect
    bitcoind.generate(21).unwrap();
    assert_eq!(bitcoind.get_chain_tip().unwrap(), 21);

    connect(&bitcoind, &utreexod).unwrap();

    wait_for_height(&utreexod, 21).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 21);
}

// Doesn't work
// BitcoinD needs to mine and only then connect to UtreexoD.
// UtreexoD appears to not sync headers after the initial handshake.
#[test]
#[ignore]
fn test_bitcoind_utreexod_chain_sync() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    connect(&bitcoind, &utreexod).unwrap();

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
    wait_for_height(&utreexod, 10).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 10);

    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 20).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 20);
}
