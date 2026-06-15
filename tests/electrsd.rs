// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Integration Tests between [`ElectrsD`] and [`BitcoinD`].

#![cfg(all(feature = "bitcoind_31_0", feature = "electrs_0_11_1"))]

use corepc_client::bitcoin::Amount;
use electrum_client::ElectrumApi;
use halfin::bitcoind::BitcoinD;
use halfin::electrsd::ElectrsD;
use halfin::electrsd::wait_for_electrs_mempool_tx;
use halfin::electrsd::wait_for_electrs_tip;
use halfin::electrsd::wait_for_electrs_to_catch_up;
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

    electrsd.get_electrum_client().ping().unwrap();

    info!("PID: {}", electrsd.get_pid());
    info!("Working Directory: {:?}", electrsd.get_working_directory());
    info!("Electrum Socket: {}", electrsd.electrum_socket());
    info!("Monitoring Socket: {}", electrsd.monitoring_socket());
}

/// Verify that [`ElectrsD`] tracks mempool transactions.
#[test]
fn test_electrsd_sees_mempool_transactions() {
    const BLOCK_COUNT: u32 = 101;

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    electrsd.get_electrum_client().ping().unwrap();
    wait_for_electrs_to_catch_up(&electrsd, &bitcoind).unwrap();

    let address = bitcoind
        .get_rpc_client()
        .get_new_address(None, None)
        .unwrap()
        .address()
        .unwrap()
        .assume_checked();
    let script_pubkey = address.script_pubkey();
    let txid = bitcoind
        .get_rpc_client()
        .send_to_address(&address, Amount::from_int_btc(1))
        .unwrap()
        .txid()
        .unwrap();
    electrsd.trigger().unwrap();

    wait_for_electrs_mempool_tx(&electrsd, &script_pubkey, txid).unwrap();
}

/// Verify that [`ElectrsD`] repeatedly syncs to [`BitcoinD`]'s chain tip.
#[test]
fn test_electrsd_syncs_blocks() {
    const BLOCK_COUNT: u32 = 1;
    const SYNC_STRESS_BLOCK_BATCHES: &[u32] = &[1, 2, 5];

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    wait_for_electrs_to_catch_up(&electrsd, &bitcoind).unwrap();

    let mut expected_height = BLOCK_COUNT;
    for batch in SYNC_STRESS_BLOCK_BATCHES {
        bitcoind.generate(*batch).unwrap();
        electrsd.trigger().unwrap();

        expected_height += batch;
        wait_for_electrs_tip(&electrsd, expected_height).unwrap();
        wait_for_electrs_to_catch_up(&electrsd, &bitcoind).unwrap();
    }
}
