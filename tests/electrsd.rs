// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Integration Tests between [`ElectrsD`] and [`BitcoinD`].

#![cfg(all(feature = "bitcoind", feature = "electrs"))]

use corepc_client::bitcoin::Amount;
use electrum_client::ElectrumApi;
use halfin::bitcoind::BitcoinD;
use halfin::electrsd::ELECTRS_INDEXING_TIMEOUT;
use halfin::electrsd::ElectrsD;
use tracing::Level;
use tracing::info;

/// Verify that [`ElectrsD`] starts and accepts Electrum requests.
#[test]
fn test_electrsd_spawns() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    electrsd.client.ping().unwrap();

    info!("PID: {}", electrsd.get_pid());
    info!("Working Directory: {:?}", electrsd.get_working_directory());
    info!("Electrum Socket: {}", electrsd.electrum_socket());
    info!(
        "Electrum Server Protocol Version: {}",
        electrsd.client.server_features().unwrap().protocol_max
    );
    info!("Monitoring Socket: {}", electrsd.monitoring_socket());
}

/// Verify that [`ElectrsD`] tracks mempool transactions.
#[test]
fn test_electrsd_sees_mempool_transactions() {
    const BLOCK_COUNT: u32 = 101;

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    electrsd.client.ping().unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

    let address = bitcoind
        .client
        .get_new_address(None, None)
        .unwrap()
        .address()
        .unwrap()
        .assume_checked();
    let script_pubkey = address.script_pubkey();
    let txid = bitcoind
        .client
        .send_to_address(&address, Amount::from_int_btc(1))
        .unwrap()
        .txid()
        .unwrap();
    electrsd.trigger().unwrap();

    electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(ELECTRS_INDEXING_TIMEOUT))
        .unwrap();
}

/// Verify that [`ElectrsD`] repeatedly syncs to [`BitcoinD`]'s chain tip.
#[test]
fn test_electrsd_syncs_blocks() {
    const BLOCK_COUNT: u32 = 1;
    const SYNC_STRESS_BLOCK_BATCHES: &[u32] = &[1, 2, 5];

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

    let mut exp_height = BLOCK_COUNT;
    for batch in SYNC_STRESS_BLOCK_BATCHES {
        bitcoind.generate(*batch).unwrap();
        electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

        exp_height += batch;
        let exp_hash = bitcoind.get_block_hash(exp_height).unwrap();
        electrsd
            .wait_until_tip(exp_height, exp_hash, Some(ELECTRS_INDEXING_TIMEOUT))
            .unwrap();
        electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    }
}

/// Verify that [`ElectrsD`] follows the replacement tip after a reorg.
#[test]
fn test_electrsd_reindexes_reorgs() {
    let bitcoind = BitcoinD::new().unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let hash = bitcoind.get_block_hash(height).unwrap();

    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, height);
    assert_eq!(tip.header.block_hash(), hash);

    // Invalidate the latest block
    bitcoind.invalidate_blocks(1).unwrap();

    // Mine a new block simulating a chain reorg
    bitcoind.generate(1).unwrap();

    let reorg_height = bitcoind.get_chain_tip().unwrap();
    let reorg_hash = bitcoind.get_block_hash(reorg_height).unwrap();

    assert_ne!(hash, reorg_hash);
    assert_eq!(height, reorg_height);

    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let reorg_tip = electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(reorg_tip.height as u32, reorg_height);
    assert_eq!(reorg_tip.header.block_hash(), reorg_hash);
}
