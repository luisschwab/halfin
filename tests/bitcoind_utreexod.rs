// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`BitcoinD`] and [`UtreexoD`].

#![cfg(all(feature = "bitcoind_31_0", feature = "utreexod_0_5_2"))]

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

/// Verify that blocks mined on [`BitcoinD`] *after* a [`UtreexoD`] peer is
/// connected propagate live to that peer.
///
/// The chain must be bootstrapped with at least one block before connecting.
/// If [`BitcoinD`] is in initial block download and never establishes the
/// header-sync relationship, and blocks mined afterwards are never announced.
/// Mining one block first takes [`BitcoinD`] out of IBD, and thenlive block
/// relay works.
#[ignore]
#[test]
fn test_bitcoind_utreexod_chain_sync() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    // Bootstrap out of genesis, then connect and sync.
    bitcoind.generate(1).unwrap();
    connect(&bitcoind, &utreexod).unwrap();
    wait_for_height(&utreexod, 1).unwrap();

    // Blocks mined after connecting must propagate live.
    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 11).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 11);

    bitcoind.generate(10).unwrap();
    wait_for_height(&utreexod, 21).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 21);
}
