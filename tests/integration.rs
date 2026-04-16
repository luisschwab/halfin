// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`BitcoinD`] and [`UtreexoD`].

use halfin::bitcoind::BitcoinD;
use halfin::utreexod::UtreexoD;
use halfin::wait_for_height;

/// Verify that [`BitcoinD`] and [`UtreexoD`] can connect to each other.
#[test]
fn test_bitcoind_utreexod_addnode() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);
    assert_eq!(utreexod.get_peer_count().unwrap(), 0);

    bitcoind.add_peer(utreexod.get_p2p_socket()).unwrap();

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
    assert_eq!(bitcoind.get_height().unwrap(), 21);

    utreexod.add_peer(bitcoind.get_p2p_socket()).unwrap();

    wait_for_height(&utreexod, 21).unwrap();
    assert_eq!(utreexod.get_height().unwrap(), 21);
}

// Doesn't work
// BitcoinD needs to mine and only then connect to UtreexoD.
// UtreexoD appears to not sync headers after the initial handshake.
#[test]
#[ignore]
fn test_bitcoind_utreexod_chain_sync() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

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
    wait_for_height(&utreexod, 10).unwrap();
    assert_eq!(utreexod.get_height().unwrap(), 10);

    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 20).unwrap();
    assert_eq!(utreexod.get_height().unwrap(), 20);
}
