// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the [`Indexer`](halfin::indexer::Indexer) trait.

#![cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]

use core::time::Duration;

use corepc_client::bitcoin::Amount;
use electrum_client::ElectrumApi;
use halfin::bitcoind::BitcoinD;
use halfin::electrsd::ElectrsD;
use halfin::electrsd::ElectrsDConf;
use halfin::electrumxd::ElectrumxD;
use halfin::electrumxd::ElectrumxDConf;
use halfin::indexer::Indexer;

/// Verify that both Electrum servers implement the complete [`Indexer`](halfin::indexer::Indexer)
/// API.
#[test]
fn test_indexer_trait() {
    const BLOCK_COUNT: u32 = 101;
    const TIMEOUT: Duration = Duration::from_secs(30);

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(BLOCK_COUNT).unwrap();

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

    let electrsd_conf = ElectrsDConf::default();
    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &electrsd_conf).unwrap();
    assert!(!<ElectrsD as Indexer>::get_name().is_empty());
    assert!(!<ElectrsD as Indexer>::get_bin_name().is_empty());
    assert!(Indexer::get_pid(&electrsd) > 0);
    assert!(Indexer::get_working_directory(&electrsd).is_dir());
    assert_eq!(Indexer::get_config(&electrsd), &electrsd_conf);
    assert_eq!(
        Indexer::electrum_url(&electrsd),
        Indexer::electrum_socket(&electrsd).to_string()
    );
    Indexer::get_electrum_client(&electrsd).ping().unwrap();
    Indexer::trigger(&electrsd).unwrap();
    Indexer::wait_until_caught_up(&electrsd, &bitcoind, Some(TIMEOUT)).unwrap();
    let height = bitcoind.get_chain_tip().unwrap();
    let hash = bitcoind.get_block_hash(height).unwrap();
    Indexer::wait_until_tip(&electrsd, height, hash, Some(TIMEOUT)).unwrap();
    Indexer::wait_until_mempool_tx(&electrsd, &script_pubkey, txid, Some(TIMEOUT)).unwrap();
    Indexer::stop(&mut electrsd).unwrap();

    let electrumxd_conf = ElectrumxDConf::default();
    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &electrumxd_conf).unwrap();
    assert!(!<ElectrumxD as Indexer>::get_name().is_empty());
    assert!(!<ElectrumxD as Indexer>::get_bin_name().is_empty());
    assert!(Indexer::get_pid(&electrumxd) > 0);
    assert!(Indexer::get_working_directory(&electrumxd).is_dir());
    assert_eq!(Indexer::get_config(&electrumxd), &electrumxd_conf);
    assert_eq!(
        Indexer::electrum_url(&electrumxd),
        Indexer::electrum_socket(&electrumxd).to_string()
    );
    Indexer::get_electrum_client(&electrumxd).ping().unwrap();
    Indexer::trigger(&electrumxd).unwrap();
    Indexer::wait_until_caught_up(&electrumxd, &bitcoind, Some(TIMEOUT)).unwrap();
    let height = bitcoind.get_chain_tip().unwrap();
    let hash = bitcoind.get_block_hash(height).unwrap();
    Indexer::wait_until_tip(&electrumxd, height, hash, Some(TIMEOUT)).unwrap();
    Indexer::wait_until_mempool_tx(&electrumxd, &script_pubkey, txid, Some(TIMEOUT)).unwrap();
    Indexer::stop(&mut electrumxd).unwrap();
}
