// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(feature = "bitcoind_31_0")]

use halfin::bitcoind::BitcoinD;
use halfin::bitcoind::get_bitcoind_path;
use halfin::connect;
use halfin::wait_for_height;

/// Verify that [`BitcoinD`] starts successfully and
/// exposes its PID, working directory, and P2P socket.
#[test]
fn test_bitcoind_starts() {
    let bin = get_bitcoind_path().unwrap();
    let bitcoind = BitcoinD::from_bin(bin).unwrap();

    println!("PID: {}", bitcoind.get_pid());
    println!("Working Directory: {:?}", bitcoind.get_working_directory());
    println!("P2P Socket: {}", bitcoind.get_p2p_socket());
}

/// Verify that `generate` mines the requested number of blocks.
#[test]
fn test_bitcoind_generate() {
    let bitcoind = BitcoinD::new().unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    assert_eq!(height, 0);

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    assert_eq!(height, 10);
}

/// Verify that [`BitcoinD::get_block_hash`] returns
/// the correct [`BlockHash`] for a given height.
#[test]
fn test_bitcoind_get_block_hash() {
    let bitcoind = BitcoinD::new().unwrap();

    let block_hashes = bitcoind.generate(10).unwrap();

    let last_block_hash = bitcoind.get_block_hash(10).unwrap();

    assert_eq!(last_block_hash, *block_hashes.last().unwrap());
}

/// Verify that two nodes can connect to each other via `connect`
/// and that the peer count reflects the new connection on both sides.
#[test]
fn test_bitcoind_addnode() {
    let bitcoind_alpha = BitcoinD::new().unwrap();
    let bitcoind_beta = BitcoinD::new().unwrap();

    assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 0);
    assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 0);

    connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

    assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 1);
    assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 1);
}

/// Verify that blocks mined on one node propagate to a peer.
#[test]
fn test_bitcoind_blocks_propagate() {
    let bitcoind_alpha = BitcoinD::new().unwrap();
    let bitcoind_beta = BitcoinD::new().unwrap();

    bitcoind_alpha.generate(21).unwrap();

    assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 21);
    assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 0);

    connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

    wait_for_height(&bitcoind_beta, 21).unwrap();
    assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 21);

    bitcoind_beta.generate(21).unwrap();
    wait_for_height(&bitcoind_alpha, 42).unwrap();
    assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 42);
}
