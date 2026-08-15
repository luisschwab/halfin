// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for [`BitcoinD`] and [`UtreexoD`].
//!
//! These tests verify peer connections and block relay between the two [`Node`] types.
//!
//! [`Node`]: halfin::node::Node

#![cfg(all(feature = "bitcoind", feature = "utreexod"))]

use halfin::node::bitcoind::BitcoinD;
use halfin::node::connect;
use halfin::node::utreexod::UtreexoD;
use halfin::node::wait_for_height;

/// Verify a connection between [`BitcoinD`] and [`UtreexoD`].
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

/// Verify block propagation from [`BitcoinD`] to a connected [`UtreexoD`].
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

/// Verify live block propagation from [`BitcoinD`] to [`UtreexoD`].
///
/// Mine one block before you connect the [`Node`](halfin::node::Node) implementations.
/// This action removes [`BitcoinD`] from initial block download (IBD).
/// The [`Node`](halfin::node::Node) can then announce new blocks to its peer.
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
