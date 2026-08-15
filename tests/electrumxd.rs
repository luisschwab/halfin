// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Integration Tests between [`ElectrumxD`] and [`BitcoinD`].

#![cfg(all(feature = "bitcoind", feature = "electrumx"))]

use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::sync::Mutex;

use corepc_client::bitcoin::Amount;
use electrum_client::ElectrumApi;
use halfin::Error;
use halfin::indexer::IndexerError;
use halfin::indexer::electrumxd::ELECTRUMX_INDEXING_TIMEOUT;
use halfin::indexer::electrumxd::ElectrumxD;
use halfin::node::bitcoind::BitcoinD;
use tracing::Level;
use tracing::info;

static ELECTRUMX_TEST_LOCK: Mutex<()> = Mutex::new(());

fn electrumx_test_lock() -> std::sync::MutexGuard<'static, ()> {
    ELECTRUMX_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Restore `PYTHON` after a test changes it.
struct PythonEnvGuard {
    original: Option<OsString>,
}

impl PythonEnvGuard {
    /// Set `PYTHON` for the duration of a test.
    fn set(value: &OsStr) -> Self {
        let original = env::var_os("PYTHON");

        // SAFETY: every test in this binary holds `ELECTRUMX_TEST_LOCK` while changing or reading
        // `PYTHON` through a bundled `ElectrumxD` constructor.
        unsafe {
            env::set_var("PYTHON", value);
        }

        Self { original }
    }
}

impl Drop for PythonEnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is dropped while its test still holds `ELECTRUMX_TEST_LOCK`.
        unsafe {
            match &self.original {
                Some(value) => env::set_var("PYTHON", value),
                None => env::remove_var("PYTHON"),
            }
        }
    }
}

/// Verify that `PYTHON` takes precedence and missing overrides produce an actionable error.
#[test]
fn bundled_constructor_rejects_missing_python_override() {
    let _guard = electrumx_test_lock();
    let bitcoind = BitcoinD::new().unwrap();
    let _python = PythonEnvGuard::set(OsStr::new("halfin-python-command-that-does-not-exist"));

    assert!(matches!(
        ElectrumxD::new(&bitcoind),
        Err(Error::Indexer(IndexerError::InvalidPython(description)))
            if description.contains("failed to run Python version check")
    ));
}

/// Verify that custom executables do not inherit the bundled launcher's Python requirement.
#[test]
fn custom_binary_constructor_skips_python_preflight() {
    let _guard = electrumx_test_lock();
    let bitcoind = BitcoinD::new().unwrap();
    let _python = PythonEnvGuard::set(OsStr::new("halfin-python-command-that-does-not-exist"));

    assert!(matches!(
        ElectrumxD::from_bin("missing-electrumx", &bitcoind),
        Err(Error::BinaryPathNotAbsolute { .. })
    ));
}

/// Verify that [`ElectrumxD`] starts and accepts Electrum requests.
#[test]
fn test_electrumxd_spawns() {
    let _guard = electrumx_test_lock();

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
    info!("Electrum Socket: {}", electrumxd.get_electrum_socket());
    info!(
        "Electrum Server Protocol Version: {}",
        electrumxd.client.server_features().unwrap().protocol_max
    );
    info!("Admin RPC Socket: {}", electrumxd.get_rpc_socket());
}

/// Verify that [`ElectrumxD`] tracks mempool transactions.
#[test]
fn test_electrumxd_sees_mempool_transactions() {
    const BLOCK_COUNT: u32 = 101;

    let _guard = electrumx_test_lock();

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

    let _guard = electrumx_test_lock();

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
    let _guard = electrumx_test_lock();

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
