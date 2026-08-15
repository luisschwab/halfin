// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared integration tests for [`Indexer`] implementations.
//!
//! These tests apply the [`Indexer`] interface to each enabled implementation.
//!
//! [`Indexer`]: crate::indexer::Indexer

#[cfg(feature = "bitcoind")]
use core::fmt::Debug;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use std::sync::Condvar;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use std::sync::Mutex;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
use corepc_client::bitcoin::BlockHash;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Script;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::ScriptBuf;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Txid;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;

#[cfg(feature = "bitcoind")]
use super::Indexer;
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
use crate::CONFIRMATION_BLOCK_COUNT;
#[cfg(feature = "bitcoind")]
use crate::MATURE_COINBASE_BLOCK_COUNT;
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
use crate::indexer::electrsd::ElectrsD;
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
use crate::indexer::electrsd::ElectrsDConf;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use crate::indexer::electrumxd::ElectrumxD;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use crate::indexer::electrumxd::ElectrumxDConf;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;

/// Maximum number of concurrent [`ElectrumxD`] tests.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
const ELECTRUMX_TEST_CONCURRENCY: usize = 2;

/// State that limits concurrent [`ElectrumxD`] tests.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
static ELECTRUMX_TEST_STATE: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// Permit to run one [`ElectrumxD`] test.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
#[derive(Debug)]
pub(super) struct ElectrumxTestPermit;

#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
impl Drop for ElectrumxTestPermit {
    fn drop(&mut self) {
        let (active, available) = &ELECTRUMX_TEST_STATE;
        let mut active = active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active -= 1;
        available.notify_one();
    }
}

/// Wait for permission to run an [`ElectrumxD`] test.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
pub(super) fn electrumx_test_permit() -> ElectrumxTestPermit {
    let (active, available) = &ELECTRUMX_TEST_STATE;
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    while *active >= ELECTRUMX_TEST_CONCURRENCY {
        active = available
            .wait(active)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }

    *active += 1;
    ElectrumxTestPermit
}

/// Consensus values returned by an [`Indexer`] for one script and transaction.
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
#[derive(Debug, PartialEq, Eq)]
struct IndexedValues {
    /// Hash of the block at the selected height.
    block_hash: BlockHash,

    /// Transaction history without optional server-specific fee metadata.
    history: Vec<(Txid, i32)>,

    /// Confirmed and unconfirmed script balances.
    balance: (u64, i64),

    /// Unspent outputs as transaction ID, height, output index, and value.
    unspent: Vec<(Txid, usize, usize, u64)>,

    /// Serialized transaction.
    transaction: Vec<u8>,
}

/// Read comparable consensus values from an [`Indexer`].
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
fn indexed_values(
    indexer: &impl Indexer,
    height: u32,
    script_pubkey: &Script,
    txid: Txid,
) -> IndexedValues {
    let client = Indexer::get_electrum_client(indexer);
    let height = usize::try_from(height).unwrap();

    let mut history = client
        .script_get_history(script_pubkey)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tx_hash, entry.height))
        .collect::<Vec<_>>();
    history.sort_unstable();

    let balance = client.script_get_balance(script_pubkey).unwrap();

    let mut unspent = client
        .script_list_unspent(script_pubkey)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tx_hash, entry.height, entry.tx_pos, entry.value))
        .collect::<Vec<_>>();
    unspent.sort_unstable();

    IndexedValues {
        block_hash: client.block_header(height).unwrap().block_hash(),
        history,
        balance: (balance.confirmed, balance.unconfirmed),
        unspent,
        transaction: client.transaction_get_raw(&txid).unwrap(),
    }
}

/// Poll [`ElectrumxD`] until it reports a transaction at its confirmation height.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
pub(super) fn wait_until_electrumx_confirms_transaction(
    electrumxd: &ElectrumxD,
    script_pubkey: &Script,
    txid: Txid,
    confirmation_height: u32,
) {
    let confirmation_height = i32::try_from(confirmation_height).unwrap();
    let start = std::time::Instant::now();
    loop {
        let heights = electrumxd
            .client
            .script_get_history(script_pubkey)
            .unwrap()
            .into_iter()
            .filter_map(|entry| (entry.tx_hash == txid).then_some(entry.height))
            .collect::<Vec<_>>();
        if heights == [confirmation_height] {
            return;
        }
        assert!(
            start.elapsed() < crate::indexer::electrumxd::ELECTRUMX_INDEXING_TIMEOUT,
            "{} did not report transaction {txid} at height {confirmation_height}: heights={heights:?}",
            ElectrumxD::get_name()
        );
        std::thread::sleep(2 * crate::POLL_INTERVAL);
    }
}

/// Create a mempool transaction that pays a new address.
#[cfg(feature = "bitcoind")]
fn build_transaction(bitcoind: &BitcoinD) -> (ScriptBuf, Txid) {
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let txid = bitcoind
        .client
        .send_to_address(&address, Amount::from_int_btc(1))
        .unwrap()
        .txid()
        .unwrap();

    (script_pubkey, txid)
}

/// Verify the complete [`Indexer`] interface.
#[cfg(feature = "bitcoind")]
fn assert_indexer_interface<I>(
    indexer: &mut I,
    config: &I::Config,
    bitcoind: &BitcoinD,
    script_pubkey: &Script,
    txid: Txid,
) where
    I: Indexer,
    I::Config: Debug + PartialEq,
{
    assert!(!I::get_name().is_empty());
    assert!(!I::get_bin_name().is_empty());
    assert!(Indexer::get_pid(indexer) > 0);
    assert!(Indexer::get_working_directory(indexer).is_dir());
    assert_eq!(Indexer::get_config(indexer), config);

    let socket = Indexer::get_electrum_socket(indexer);
    assert!(socket.ip().is_loopback());
    assert_eq!(Indexer::get_electrum_url(indexer), socket.to_string());
    Indexer::get_electrum_client(indexer).ping().unwrap();

    Indexer::trigger(indexer).unwrap();
    Indexer::wait_until_caught_up(indexer, bitcoind, None).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();
    Indexer::wait_until_tip(indexer, height, block_hash, None).unwrap();
    Indexer::wait_until_mempool_tx(indexer, script_pubkey, txid, None).unwrap();
    Indexer::stop(indexer).unwrap();
}

/// Verify the [`Indexer`] interface for [`ElectrsD`].
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
#[test]
fn electrsd_implements_indexer() {
    let bitcoind = BitcoinD::new().unwrap();
    let (script_pubkey, txid) = build_transaction(&bitcoind);
    let config = ElectrsDConf::default();
    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &config).unwrap();

    assert_indexer_interface(&mut electrsd, &config, &bitcoind, &script_pubkey, txid);
}

/// Verify the [`Indexer`] interface for [`ElectrumxD`].
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
#[test]
fn electrumxd_implements_indexer() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    let (script_pubkey, txid) = build_transaction(&bitcoind);
    let config = ElectrumxDConf::default();
    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &config).unwrap();

    assert_indexer_interface(&mut electrumxd, &config, &bitcoind, &script_pubkey, txid);
}

/// Verify that [`ElectrsD`] and [`ElectrumxD`] index the same values.
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
#[test]
fn electrsd_and_electrumxd_index_same_values() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let amount = Amount::from_int_btc(1);
    let txid = bitcoind
        .client
        .send_to_address(&address, amount)
        .unwrap()
        .txid()
        .unwrap();

    electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, None)
        .unwrap();
    electrumxd
        .wait_until_mempool_tx(&script_pubkey, txid, None)
        .unwrap();

    let electrs_mempool = indexed_values(&electrsd, height, &script_pubkey, txid);
    let electrumx_mempool = indexed_values(&electrumxd, height, &script_pubkey, txid);
    assert_eq!(electrs_mempool, electrumx_mempool);
    assert_eq!(electrs_mempool.block_hash, block_hash);
    assert_eq!(electrs_mempool.history, [(txid, 0)]);
    assert_eq!(
        electrs_mempool.balance,
        (0, i64::try_from(amount.to_sat()).unwrap())
    );
    assert_eq!(electrs_mempool.unspent.len(), 1);
    assert_eq!(electrs_mempool.unspent[0].0, txid);
    assert_eq!(electrs_mempool.unspent[0].1, 0);
    assert_eq!(electrs_mempool.unspent[0].3, amount.to_sat());

    bitcoind.generate(CONFIRMATION_BLOCK_COUNT).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let confirmation_height = height + 1;
    wait_until_electrumx_confirms_transaction(
        &electrumxd,
        &script_pubkey,
        txid,
        confirmation_height,
    );
    let height = bitcoind.get_chain_tip().unwrap();
    let electrs_confirmed = indexed_values(&electrsd, height, &script_pubkey, txid);
    let electrumx_confirmed = indexed_values(&electrumxd, height, &script_pubkey, txid);
    assert_eq!(electrs_confirmed, electrumx_confirmed);
    assert_eq!(
        electrs_confirmed.block_hash,
        bitcoind.get_block_hash(height).unwrap()
    );
    assert_eq!(
        electrs_confirmed.history,
        [(txid, i32::try_from(confirmation_height).unwrap())]
    );
    assert_eq!(electrs_confirmed.balance, (amount.to_sat(), 0));
    assert_eq!(electrs_confirmed.unspent.len(), 1);
    assert_eq!(electrs_confirmed.unspent[0].0, txid);
    assert_eq!(
        electrs_confirmed.unspent[0].1,
        usize::try_from(confirmation_height).unwrap()
    );
    assert_eq!(electrs_confirmed.unspent[0].3, amount.to_sat());
    assert_eq!(electrs_confirmed.transaction, electrs_mempool.transaction);

    let confirmation_height = usize::try_from(confirmation_height).unwrap();
    let electrs_merkle = electrsd
        .client
        .transaction_get_merkle(&txid, confirmation_height)
        .unwrap();
    let electrumx_merkle = electrumxd
        .client
        .transaction_get_merkle(&txid, confirmation_height)
        .unwrap();
    assert_eq!(electrs_merkle.block_height, confirmation_height);
    assert_eq!(electrumx_merkle.block_height, confirmation_height);
    assert_eq!(electrs_merkle.pos, electrumx_merkle.pos);
    assert_eq!(electrs_merkle.merkle, electrumx_merkle.merkle);
}
