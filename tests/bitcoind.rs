// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Integration Tests for [`BitcoinD`].

#![cfg(feature = "bitcoind_31_0")]

use corepc_client::bitcoin::Amount;
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

/// Verify that `generatetoaddress` mines the
/// requested number of blocks to the specified [`Address`].
#[test]
fn test_bitcoind_generate_to_address() {
    const BLOCK_COUNT: u32 = 21;

    let bitcoind = BitcoinD::new().unwrap();

    let address = bitcoind
        .get_rpc_client()
        .get_new_address(None, None)
        .unwrap()
        .address()
        .unwrap()
        .assume_checked();

    bitcoind.generate_to_address(BLOCK_COUNT, &address).unwrap();

    let address_desc = format!("addr({})", address);
    let address_balance = bitcoind
        .get_rpc_client()
        .scan_tx_out_set_start(&[&address_desc])
        .unwrap()
        .total_amount;

    assert_eq!(
        Amount::from_btc(address_balance).unwrap(),
        Amount::from_int_btc(BLOCK_COUNT as u64 * 50)
    );
}

#[test]
fn test_bitcoind_get_filter_height() {
    const BLOCK_COUNT: u32 = 21;

    let bitcoind = BitcoinD::new().unwrap();

    bitcoind.generate(BLOCK_COUNT).unwrap();

    assert_eq!(BLOCK_COUNT, bitcoind.get_filter_tip().unwrap());
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
