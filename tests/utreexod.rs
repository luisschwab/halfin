// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for [`UtreexoD`].

#![cfg(feature = "utreexod_0_5_1")]

use halfin::connect;
use halfin::utreexod::UtreexoD;
use halfin::utreexod::get_utreexod_path;
use halfin::wait_for_filter_height;
use halfin::wait_for_height;

/// Verify that [`UtreexoD`] starts successfully and
/// exposes its PID, working directory, and P2P socket.
#[test]
fn test_utreexod_starts() {
    let bin_path = get_utreexod_path().unwrap();
    let utreexod = UtreexoD::from_bin(bin_path).unwrap();

    println!("PID: {}", utreexod.get_pid());
    println!("Working Directory: {:?}", utreexod.get_working_directory());
    println!("P2P Socket: {}", utreexod.get_p2p_socket());
}

/// Verify that `generate` mines the requested number of blocks.
#[test]
fn test_utreexod_generate() {
    let utreexod = UtreexoD::new().unwrap();

    let height = utreexod.get_chain_tip().unwrap();
    assert_eq!(height, 0);

    utreexod.generate(10).unwrap();

    let height = utreexod.get_chain_tip().unwrap();
    assert_eq!(height, 10);
}

#[test]
fn test_bitcoind_get_filter_height() {
    const BLOCK_COUNT: u32 = 21;

    let utreexod = UtreexoD::new().unwrap();

    utreexod.generate(BLOCK_COUNT).unwrap();
    wait_for_filter_height(&utreexod, BLOCK_COUNT).unwrap();

    assert_eq!(BLOCK_COUNT, utreexod.get_filter_tip().unwrap());
}

/// Verify that [`UtreexoD::get_block_hash`] returns the correct hash for a given height.
#[test]
fn test_utreexod_get_block_hash() {
    let utreexod = UtreexoD::new().unwrap();

    let block_hashes = utreexod.generate(10).unwrap();

    let last_block_hash = utreexod.get_block_hash(10).unwrap();

    assert_eq!(last_block_hash, *block_hashes.last().unwrap());
}

/// Verify that two nodes can connect to each other via `connect`,
/// and that the peer count reflects the new connection on both sides.
#[test]
fn test_utreexod_addnode() {
    let utreexod_alpha = UtreexoD::new().unwrap();
    let utreexod_beta = UtreexoD::new().unwrap();

    assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 0);
    assert_eq!(utreexod_beta.get_peer_count().unwrap(), 0);

    connect(&utreexod_alpha, &utreexod_beta).unwrap();

    assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 1);
    assert_eq!(utreexod_beta.get_peer_count().unwrap(), 1);
}

/// Verify that blocks mined on one node propagate to a peer.
#[test]
fn test_utreexod_blocks_propagate() {
    let utreexod_alpha = UtreexoD::new().unwrap();
    let utreexod_beta = UtreexoD::new().unwrap();

    utreexod_alpha.generate(21).unwrap();

    assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 21);
    assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 0);

    connect(&utreexod_alpha, &utreexod_beta).unwrap();

    wait_for_height(&utreexod_beta, 21).unwrap();
    assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 21);

    utreexod_beta.generate(21).unwrap();
    wait_for_height(&utreexod_alpha, 42).unwrap();
    assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 42);
}
