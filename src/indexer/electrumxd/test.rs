// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`ElectrumxD`].

#[cfg(feature = "bitcoind")]
use std::env;
#[cfg(feature = "bitcoind")]
use std::process::Command;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
use corepc_client::bitcoin::Network;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;
#[cfg(feature = "bitcoind")]
use tracing::Level;
#[cfg(feature = "bitcoind")]
use tracing::info;

#[cfg(feature = "bitcoind")]
use super::ELECTRUMX_INDEXING_TIMEOUT;
use super::ElectrumxD;
use super::ElectrumxDArgs;
use super::ElectrumxDConf;
#[cfg(any(feature = "florestad", feature = "utreexod"))]
use super::get_electrumx_path;
#[cfg(feature = "bitcoind")]
use crate::CONFIRMATION_BLOCK_COUNT;
use crate::Error;
#[cfg(feature = "bitcoind")]
use crate::MATURE_COINBASE_BLOCK_COUNT;
#[cfg(feature = "bitcoind")]
use crate::PERSISTENCE_BLOCK_COUNT;
use crate::SPAWN_ATTEMPTS;
#[cfg(feature = "bitcoind")]
use crate::SYNC_BLOCK_BATCHES;
#[cfg(feature = "bitcoind")]
use crate::SYNC_INITIAL_BLOCK_COUNT;
use crate::indexer::IndexerError;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::electrumx_test_permit;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::wait_until_electrumx_confirms_transaction;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

#[cfg(feature = "bitcoind")]
const PYTHON_OVERRIDE_CHILD: &str = "HALFIN_ELECTRUMX_PYTHON_OVERRIDE_CHILD";

#[cfg(feature = "bitcoind")]
const MISSING_PYTHON: &str = "invalid-python-command";

/// Return whether this process is the isolated child for `test_name`.
#[cfg(feature = "bitcoind")]
fn is_python_override_child(test_name: &str) -> bool {
    env::var(PYTHON_OVERRIDE_CHILD).is_ok_and(|value| value == test_name)
}

/// Run `test_name` in a child process with an invalid `PYTHON` override.
#[cfg(feature = "bitcoind")]
fn run_with_missing_python(test_name: &str) {
    let output = Command::new(env::current_exe().unwrap())
        .arg(test_name)
        .arg("--exact")
        .arg("--nocapture")
        .env(PYTHON_OVERRIDE_CHILD, test_name)
        .env("PYTHON", MISSING_PYTHON)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "child test failed with status={}; stdout={}; stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Verify that `PYTHON` overrides other interpreter selections.
/// Verify the error for a missing interpreter.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_bundled_constructor_rejects_missing_python_override() {
    const TEST_NAME: &str =
        "indexer::electrumxd::test::electrumxd_bundled_constructor_rejects_missing_python_override";

    if !is_python_override_child(TEST_NAME) {
        run_with_missing_python(TEST_NAME);
        return;
    }

    let bitcoind = BitcoinD::new().unwrap();

    assert!(matches!(
        ElectrumxD::new(&bitcoind),
        Err(Error::Indexer(IndexerError::InvalidPython(description)))
            if description.contains("failed to run Python version check")
    ));
}

/// Verify that custom executables do not use the Python requirement of the bundled launcher.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_custom_binary_constructor_skips_python_preflight() {
    const TEST_NAME: &str =
        "indexer::electrumxd::test::electrumxd_custom_binary_constructor_skips_python_preflight";

    if !is_python_override_child(TEST_NAME) {
        run_with_missing_python(TEST_NAME);
        return;
    }

    let bitcoind = BitcoinD::new().unwrap();

    assert!(matches!(
        ElectrumxD::from_bin("missing-electrumx", &bitcoind),
        Err(Error::BinaryPathNotAbsolute { .. })
    ));
}

/// Verify that [`ElectrumxD`] starts and accepts Electrum requests.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_accepts_bitcoind() {
    let _permit = electrumx_test_permit();

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

/// Verify that rejection of [`UtreexoD`] occurs before data directory creation.
#[cfg(feature = "utreexod")]
#[test]
fn electrumxd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrumx");
    let config = ElectrumxDConf {
        staticdir: Some(directory.clone()),
        ..ElectrumxDConf::default()
    };

    assert!(matches!(
        ElectrumxD::from_bin_with_conf(get_electrumx_path().unwrap(), &utreexod, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that rejection of [`FlorestaD`] occurs before data directory creation.
#[cfg(feature = "florestad")]
#[test]
fn electrumxd_rejects_florestad() {
    let florestad = FlorestaD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrumx");
    let config = ElectrumxDConf {
        staticdir: Some(directory.clone()),
        ..ElectrumxDConf::default()
    };

    assert!(matches!(
        ElectrumxD::from_bin_with_conf(get_electrumx_path().unwrap(), &florestad, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "FlorestaD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that [`ElectrumxD`] indexes mempool transactions.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_sees_mempool_transactions() {
    let _permit = electrumx_test_permit();

    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();

    electrumxd.client.ping().unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let address = bitcoind.client.new_address().unwrap();
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

/// Verify repeated synchronization of [`ElectrumxD`] with the [`BitcoinD`] chain tip.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_syncs_blocks() {
    let _permit = electrumx_test_permit();

    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(SYNC_INITIAL_BLOCK_COUNT).unwrap();

    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let mut height = SYNC_INITIAL_BLOCK_COUNT;
    for count in SYNC_BLOCK_BATCHES {
        bitcoind.generate(*count).unwrap();
        electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

        height += count;
        let block_hash = bitcoind.get_block_hash(height).unwrap();
        electrumxd
            .wait_until_tip(height, block_hash, Some(ELECTRUMX_INDEXING_TIMEOUT))
            .unwrap();
        electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    }
}

/// Verify that [`ElectrumxD`] uses the replacement tip after a reorganization.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_reindexes_reorgs() {
    const REORG_DEPTH: u32 = 2;

    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();

    electrumxd
        .wait_until_tip(height, block_hash, Some(ELECTRUMX_INDEXING_TIMEOUT))
        .unwrap();

    bitcoind.invalidate_blocks(REORG_DEPTH).unwrap();
    bitcoind.generate(REORG_DEPTH + 1).unwrap();

    let replacement_height = bitcoind.get_chain_tip().unwrap();
    let replacement_hash = bitcoind.get_block_hash(height).unwrap();

    assert_ne!(block_hash, replacement_hash);
    assert_eq!(height + 1, replacement_height);

    electrumxd
        .wait_until_tip(
            replacement_height,
            bitcoind.get_block_hash(replacement_height).unwrap(),
            Some(ELECTRUMX_INDEXING_TIMEOUT),
        )
        .unwrap();
    electrumxd
        .wait_until_tip(height, replacement_hash, Some(ELECTRUMX_INDEXING_TIMEOUT))
        .unwrap();
}

#[test]
fn electrumxd_configuration_defaults() {
    let config = ElectrumxDConf::default();

    assert_eq!(config.electrumx_args.coin, "Bitcoin");
    assert!(config.raw_args.is_empty());
    assert_eq!(config.max_retries, SPAWN_ATTEMPTS);
    assert_eq!(
        ElectrumxD::configured_args(&config, Network::Regtest).unwrap(),
        ["--coin", "Bitcoin", "--net", "regtest"]
    );
}

#[test]
fn electrumxd_renders_every_network() {
    let cases = [
        (Network::Bitcoin, "mainnet"),
        (Network::Testnet, "testnet"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ];

    for (network, expected) in cases {
        let config = ElectrumxDConf::default();

        assert_eq!(
            ElectrumxD::configured_args(&config, network).unwrap(),
            ["--coin", "Bitcoin", "--net", expected]
        );
    }
}

#[test]
fn electrumxd_renders_coin() {
    let config = ElectrumxDConf {
        electrumx_args: ElectrumxDArgs {
            coin: "Namecoin".to_string(),
        },
        ..ElectrumxDConf::default()
    };

    assert_eq!(
        ElectrumxD::configured_args(&config, Network::Regtest).unwrap(),
        ["--coin", "Namecoin", "--net", "regtest"]
    );
}

#[test]
fn electrumxd_rejects_owned_raw_arguments() {
    let cases = [
        "--coin",
        "--coin=Bitcoin",
        "--daemon-url",
        "--daemon-url=http://user:pass@127.0.0.1:1",
        "--db-directory",
        "--db-directory=/tmp/electrumx",
        "--net",
        "--net=testnet",
        "--peer-discovery",
        "--peer-discovery=on",
        "--no-peer-discovery",
        "--nopeer-discovery",
        "--services",
        "--services=tcp://127.0.0.1:50001",
    ];

    for arg in cases {
        let config = ElectrumxDConf {
            raw_args: vec![arg.to_string()],
            ..ElectrumxDConf::default()
        };

        assert!(matches!(
            ElectrumxD::configured_args(&config, Network::Regtest),
            Err(Error::Indexer(IndexerError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }
}

#[test]
fn electrumxd_accepts_unmodeled_raw_arguments() {
    let config = ElectrumxDConf {
        raw_args: vec![
            "--log-level=debug".to_string(),
            "--cache-mb=512".to_string(),
        ],
        ..ElectrumxDConf::default()
    };

    assert!(ElectrumxD::configured_args(&config, Network::Regtest).is_ok());
}

/// Verify process state, shutdown, and temporary cleanup.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    let config = ElectrumxDConf::default();
    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &config).unwrap();
    let directory = electrumxd.get_working_directory();

    assert!(electrumxd.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(electrumxd.get_config(), &config);
    assert!(electrumxd.get_electrum_socket().ip().is_loopback());
    assert!(electrumxd.get_rpc_socket().ip().is_loopback());
    assert_ne!(
        electrumxd.get_electrum_socket(),
        electrumxd.get_rpc_socket()
    );
    electrumxd.client.ping().unwrap();

    electrumxd.stop().unwrap();
    drop(electrumxd);
    assert!(!directory.exists());
}

/// Verify unconfirmed and confirmed balance updates.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_updates_balance_when_payment_confirms() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let amount = Amount::from_int_btc(1);
    let txid = bitcoind
        .client
        .send_to_address(&address, amount)
        .unwrap()
        .txid()
        .unwrap();

    electrumxd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(ELECTRUMX_INDEXING_TIMEOUT))
        .unwrap();
    let balance = electrumxd
        .client
        .script_get_balance(&script_pubkey)
        .unwrap();
    assert_eq!(balance.confirmed, 0);
    assert_eq!(balance.unconfirmed, i64::try_from(amount.to_sat()).unwrap());

    bitcoind.generate(CONFIRMATION_BLOCK_COUNT).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    let confirmation_height = bitcoind.get_chain_tip().unwrap() - CONFIRMATION_BLOCK_COUNT + 1;
    wait_until_electrumx_confirms_transaction(
        &electrumxd,
        &script_pubkey,
        txid,
        confirmation_height,
    );
    let balance = electrumxd
        .client
        .script_get_balance(&script_pubkey)
        .unwrap();
    assert_eq!(balance.confirmed, amount.to_sat());
    assert_eq!(balance.unconfirmed, 0);
}

/// Verify that a static directory retains indexed chain state across a restart.
#[cfg(feature = "bitcoind")]
#[test]
fn electrumxd_static_directory_restores_indexed_state() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(PERSISTENCE_BLOCK_COUNT).unwrap();

    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrumx");
    let config = ElectrumxDConf {
        staticdir: Some(directory.clone()),
        ..ElectrumxDConf::default()
    };

    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &config).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrumxd.stop().unwrap();
    drop(electrumxd);

    assert!(directory.is_dir());

    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &config).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrumxd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        tip.header.block_hash(),
        bitcoind.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap()
    );
    electrumxd.stop().unwrap();
    drop(electrumxd);
    assert!(directory.is_dir());
}
