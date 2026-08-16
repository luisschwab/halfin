// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`ElectrsD`].

use std::io::Error as IoError;
use std::io::ErrorKind;
use std::sync::Arc;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
use corepc_client::bitcoin::Network;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
#[cfg(feature = "bitcoind")]
use tracing::Level;
#[cfg(feature = "bitcoind")]
use tracing::info;

#[cfg(feature = "bitcoind")]
use super::ELECTRS_INDEXING_TIMEOUT;
use super::ElectrsD;
use super::ElectrsDConf;
use super::is_incomplete_read;
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
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Verify that socket timeouts are treated as incomplete reads.
#[test]
fn electrsd_treats_socket_timeouts_as_incomplete_reads() {
    let error = ElectrumError::IOError(IoError::from(ErrorKind::TimedOut));
    assert!(is_incomplete_read(&error));

    let error = ElectrumError::SharedIOError(Arc::new(IoError::from(ErrorKind::TimedOut)));
    assert!(is_incomplete_read(&error));
}

/// Verify that [`ElectrsD`] uses the selected Bitcoin Core P2P port and accepts requests.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_accepts_bitcoind() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 1);

    electrsd.client.ping().unwrap();

    info!("PID: {}", electrsd.get_pid());
    info!("Working Directory: {:?}", electrsd.get_working_directory());
    info!("Electrum Socket: {}", electrsd.get_electrum_socket());
    info!(
        "Electrum Server Protocol Version: {}",
        electrsd.client.server_features().unwrap().protocol_max
    );
    info!("Monitoring Socket: {}", electrsd.get_monitoring_socket());
}

/// Verify that rejection of [`UtreexoD`] occurs before data directory creation.
#[cfg(feature = "utreexod")]
#[test]
fn electrsd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = ElectrsDConf {
        staticdir: Some(directory.clone()),
        ..ElectrsDConf::default()
    };

    assert!(matches!(
        ElectrsD::new_with_conf(&utreexod, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that rejection of [`FlorestaD`] occurs before data directory creation.
#[cfg(feature = "florestad")]
#[test]
fn electrsd_rejects_florestad() {
    let florestad = FlorestaD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = ElectrsDConf {
        staticdir: Some(directory.clone()),
        ..ElectrsDConf::default()
    };

    assert!(matches!(
        ElectrsD::new_with_conf(&florestad, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "FlorestaD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that [`ElectrsD`] indexes mempool transactions.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_sees_mempool_transactions() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
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

/// Verify repeated synchronization of [`ElectrsD`] with the [`BitcoinD`] chain tip.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_syncs_blocks() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(SYNC_INITIAL_BLOCK_COUNT).unwrap();

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

    let mut height = SYNC_INITIAL_BLOCK_COUNT;
    for count in SYNC_BLOCK_BATCHES {
        bitcoind.generate(*count).unwrap();
        electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

        height += count;
        let block_hash = bitcoind.get_block_hash(height).unwrap();
        electrsd
            .wait_until_tip(height, block_hash, Some(ELECTRS_INDEXING_TIMEOUT))
            .unwrap();
        electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    }
}

/// Verify that [`ElectrsD`] uses the replacement tip after a reorganization.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_reindexes_reorgs() {
    let bitcoind = BitcoinD::new().unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();

    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, height);
    assert_eq!(tip.header.block_hash(), block_hash);

    bitcoind.invalidate_blocks(1).unwrap();
    bitcoind.generate(1).unwrap();

    let replacement_height = bitcoind.get_chain_tip().unwrap();
    let replacement_hash = bitcoind.get_block_hash(replacement_height).unwrap();

    assert_ne!(block_hash, replacement_hash);
    assert_eq!(height, replacement_height);

    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, replacement_height);
    assert_eq!(tip.header.block_hash(), replacement_hash);
}

#[test]
fn electrsd_configuration_defaults() {
    let config = ElectrsDConf::default();

    assert!(config.raw_args.is_empty());
    assert_eq!(config.max_retries, SPAWN_ATTEMPTS);
    assert_eq!(
        ElectrsD::configured_args(&config, Network::Regtest).unwrap(),
        ["--network", "regtest"]
    );
}

#[test]
fn electrsd_renders_every_network() {
    let cases = [
        (Network::Bitcoin, "bitcoin"),
        (Network::Testnet, "testnet"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ];

    for (network, expected) in cases {
        let config = ElectrsDConf::default();

        assert_eq!(
            ElectrsD::configured_args(&config, network).unwrap(),
            ["--network", expected]
        );
    }
}

#[test]
fn electrsd_rejects_owned_raw_arguments() {
    let cases = [
        "--network",
        "--network=signet",
        "--db-dir",
        "--db-dir=/tmp/electrs",
        "--daemon-rpc-addr",
        "--daemon-rpc-addr=127.0.0.1:1",
        "--daemon-p2p-addr",
        "--daemon-p2p-addr=127.0.0.1:2",
        "--electrum-rpc-addr",
        "--electrum-rpc-addr=127.0.0.1:3",
        "--monitoring-addr",
        "--monitoring-addr=127.0.0.1:4",
        "--cookie-file",
        "--cookie-file=/tmp/.cookie",
    ];

    for arg in cases {
        let config = ElectrsDConf {
            raw_args: vec![arg.to_string()],
            ..ElectrsDConf::default()
        };

        assert!(matches!(
            ElectrsD::configured_args(&config, Network::Regtest),
            Err(Error::Indexer(IndexerError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }
}

#[test]
fn electrsd_accepts_unmodeled_raw_arguments() {
    let config = ElectrsDConf {
        raw_args: vec![
            "--log-filters=debug".to_string(),
            "--index-batch-size=100".to_string(),
        ],
        ..ElectrsDConf::default()
    };

    assert!(ElectrsD::configured_args(&config, Network::Regtest).is_ok());
}

/// Verify process state, shutdown, and temporary cleanup.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let bitcoind = BitcoinD::new().unwrap();
    let config = ElectrsDConf::default();
    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    let directory = electrsd.get_working_directory();

    assert!(electrsd.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(electrsd.get_config(), &config);
    assert!(electrsd.get_electrum_socket().ip().is_loopback());
    assert!(electrsd.get_monitoring_socket().ip().is_loopback());
    assert_ne!(
        electrsd.get_electrum_socket(),
        electrsd.get_monitoring_socket()
    );
    electrsd.client.ping().unwrap();

    electrsd.stop().unwrap();
    drop(electrsd);
    assert!(!directory.exists());
}

/// Verify confirmed and unconfirmed balances across a reorganization.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_updates_balances_across_reorganizations() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let amount = Amount::from_int_btc(1);
    let txid = bitcoind
        .client
        .send_to_address(&address, amount)
        .unwrap()
        .txid()
        .unwrap();

    let block_hash = bitcoind
        .generate(CONFIRMATION_BLOCK_COUNT)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let balance = electrsd.client.script_get_balance(&script_pubkey).unwrap();
    assert_eq!(balance.confirmed, amount.to_sat());
    assert_eq!(balance.unconfirmed, 0);

    bitcoind
        .invalidate_blocks(CONFIRMATION_BLOCK_COUNT)
        .unwrap();

    let mining_address = bitcoind.client.new_address().unwrap().to_string();
    for _ in 0..=CONFIRMATION_BLOCK_COUNT {
        bitcoind
            .client
            .generate_block(&mining_address, &[], true)
            .unwrap();
    }
    let replacement_hash = bitcoind
        .get_block_hash(MATURE_COINBASE_BLOCK_COUNT + 1)
        .unwrap();
    assert_ne!(replacement_hash, block_hash);

    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(ELECTRS_INDEXING_TIMEOUT))
        .unwrap();
    let balance = electrsd.client.script_get_balance(&script_pubkey).unwrap();
    assert_eq!(balance.confirmed, 0);
    assert_eq!(balance.unconfirmed, i64::try_from(amount.to_sat()).unwrap());

    bitcoind.generate(CONFIRMATION_BLOCK_COUNT).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let balance = electrsd.client.script_get_balance(&script_pubkey).unwrap();
    assert_eq!(balance.confirmed, amount.to_sat());
    assert_eq!(balance.unconfirmed, 0);
}

/// Verify that a static directory retains indexed chain state across a restart.
#[cfg(feature = "bitcoind")]
#[test]
fn electrsd_static_directory_restores_indexed_state() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(PERSISTENCE_BLOCK_COUNT).unwrap();

    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = ElectrsDConf {
        staticdir: Some(directory.clone()),
        ..ElectrsDConf::default()
    };

    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrsd.stop().unwrap();
    drop(electrsd);

    assert!(directory.is_dir());

    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    let tip = electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        tip.header.block_hash(),
        bitcoind.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap()
    );
    electrsd.stop().unwrap();
    drop(electrsd);
    assert!(directory.is_dir());
}
