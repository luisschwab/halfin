// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Integration Tests between [`ElectrumxD`] and [`BitcoinD`].

#![cfg(all(feature = "bitcoind", feature = "electrumx"))]

use std::sync::Mutex;

use corepc_client::bitcoin::Amount;
use electrum_client::ElectrumApi;
use halfin::bitcoind::BitcoinD;
use halfin::electrumxd::ELECTRUMX_INDEXING_TIMEOUT;
use halfin::electrumxd::ElectrumxD;
use tracing::Level;
use tracing::info;

static ELECTRUMX_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Verify that [`ElectrumxD`] starts and accepts Electrum requests.
#[test]
fn test_electrumxd_spawns() {
    let _guard = ELECTRUMX_TEST_LOCK.lock().unwrap();

    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();

    electrumxd.client.ping().unwrap();

    info!("PID: {}", electrumxd.get_pid());
    info!(
        "Working Directory: {:?}",
        electrumxd.get_working_directory()
    );
    info!("Electrum Socket: {}", electrumxd.electrum_socket());
    info!(
        "Electrum Server Protocol Version: {}",
        electrumxd.client.server_features().unwrap().protocol_max
    );
    info!("Admin RPC Socket: {}", electrumxd.rpc_socket());
}

/// Verify that [`ElectrumxD`] tracks mempool transactions.
#[test]
fn test_electrumxd_sees_mempool_transactions() {
    const BLOCK_COUNT: u32 = 101;

    let _guard = ELECTRUMX_TEST_LOCK.lock().unwrap();

    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();

    electrumxd.client.ping().unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

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

    electrumxd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(ELECTRUMX_INDEXING_TIMEOUT))
        .unwrap();
}

/// Verify that [`ElectrumxD`] repeatedly syncs to [`BitcoinD`]'s chain tip.
#[test]
fn test_electrumxd_syncs_blocks() {
    const BLOCK_COUNT: u32 = 1;
    const SYNC_STRESS_BLOCK_BATCHES: &[u32] = &[1, 2, 5];

    let _guard = ELECTRUMX_TEST_LOCK.lock().unwrap();

    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();

    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let mut exp_height = BLOCK_COUNT;
    for batch in SYNC_STRESS_BLOCK_BATCHES {
        bitcoind.generate(*batch).unwrap();
        electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

        exp_height += batch;
        let exp_hash = bitcoind.get_block_hash(exp_height).unwrap();
        electrumxd
            .wait_until_tip(exp_height, exp_hash, Some(ELECTRUMX_INDEXING_TIMEOUT))
            .unwrap();
        electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    }
}

/// Verify that [`ElectrumxD`] follows the replacement tip after a reorg.
#[test]
#[ignore = "ElectrumX same-height reorg handling is shitty"]
fn test_electrumxd_reindexes_reorgs() {
    let _guard = ELECTRUMX_TEST_LOCK.lock().unwrap();

    let bitcoind = BitcoinD::new().unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let hash = bitcoind.get_block_hash(height).unwrap();

    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrumxd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, height);
    assert_eq!(tip.header.block_hash(), hash);

    bitcoind.invalidate_blocks(1).unwrap();
    bitcoind.generate(1).unwrap();

    let reorg_height = bitcoind.get_chain_tip().unwrap();
    let reorg_hash = bitcoind.get_block_hash(reorg_height).unwrap();

    assert_ne!(hash, reorg_hash);
    assert_eq!(height, reorg_height);

    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    let reorg_tip = electrumxd.client.block_headers_subscribe().unwrap();
    assert_eq!(reorg_tip.height as u32, reorg_height);
    assert_eq!(reorg_tip.header.block_hash(), reorg_hash);
}
