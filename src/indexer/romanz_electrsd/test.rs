// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`RomanzElectrsD`].

use core::time::Duration;
use std::io::Error as IoError;
use std::io::ErrorKind;
#[cfg(feature = "bitcoind")]
use std::io::Write;
#[cfg(any(feature = "bitcoind", unix))]
use std::net::TcpListener;
#[cfg(unix)]
use std::process::Command;
use std::sync::Arc;
#[cfg(feature = "bitcoind")]
use std::thread::JoinHandle;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::ScriptBuf;
use corepc_client::bitcoin::Txid;
use corepc_client::bitcoin::consensus::serialize;
use corepc_client::bitcoin::constants::genesis_block;
use corepc_client::bitcoin::hashes::Hash;
use corepc_client::bitcoin::hex::DisplayHex;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;
use electrum_client::Error as ElectrumError;
use electrum_client::HeaderNotification;
#[cfg(feature = "bitcoind")]
use serde_json::Value;
use tracing::Level;
#[cfg(feature = "bitcoind")]
use tracing::info;

#[cfg(feature = "bitcoind")]
use super::RomanzElectrsD;
use super::RomanzElectrsDConf;
#[cfg(feature = "bitcoind")]
use super::electrs_header_matches;
use super::electrs_header_matches_with;
use super::get_romanz_electrs_path;
use super::is_incomplete_read;
use super::unresponsive_indexer;
#[cfg(feature = "bitcoind")]
use crate::CONFIRMATION_BLOCK_COUNT;
use crate::Error;
#[cfg(feature = "bitcoind")]
use crate::INDEXING_TIMEOUT;
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
use crate::indexer::test::FakeNode;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::read_scripted_electrum_request;
use crate::indexer::test::scripted_electrum_client;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::scripted_electrum_reader;
#[cfg(any(feature = "bitcoind", unix))]
use crate::indexer::test::scripted_electrum_socket;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::stalled_electrum_socket;
#[cfg(unix)]
use crate::indexer::test::test_program;
use crate::node::PruneMode;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "btcd")]
use crate::node::btcd::BtcD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Start an Electrum server that queues an invalid raw header during two ping calls.
#[cfg(feature = "bitcoind")]
fn electrum_server_with_invalid_queued_header(
    initial_header: Value,
    notification_height: u32,
) -> (core::net::SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = scripted_electrum_reader(&stream);
        let Some(version_request) = read_scripted_electrum_request(&mut reader) else {
            return;
        };
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "id": version_request["id"].clone(),
                "result": ["halfin-test", "1.4"]
            })
        )
        .unwrap();

        let Some(subscribe_request) = read_scripted_electrum_request(&mut reader) else {
            return;
        };
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "id": subscribe_request["id"].clone(),
                "result": initial_header
            })
        )
        .unwrap();

        for ping_index in 0..2 {
            let Some(ping_request) = read_scripted_electrum_request(&mut reader) else {
                return;
            };
            if ping_index == 0 {
                writeln!(
                    stream,
                    "{}",
                    serde_json::json!({
                        "method": "blockchain.headers.subscribe",
                        "params": [{ "height": notification_height, "hex": "00" }]
                    })
                )
                .unwrap();
            }
            writeln!(
                stream,
                "{}",
                serde_json::json!({ "id": ping_request["id"].clone(), "result": null })
            )
            .unwrap();
        }
    });
    (socket, handle)
}

/// Verify binary-path validation and zero start attempts.
#[test]
fn romanz_electrsd_validates_binary_path_and_start_attempts() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));

    let error = RomanzElectrsD::from_bin("romanz_electrs", &node).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotAbsolute { .. }));

    let root = tempfile::tempdir().unwrap();
    let error = RomanzElectrsD::from_bin(root.path().join("missing-electrs"), &node).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotFile { .. }));

    node.write_cookie("user:password");
    let config = RomanzElectrsDConf {
        max_retries: 0,
        ..RomanzElectrsDConf::default()
    };
    let error =
        RomanzElectrsD::from_bin_with_conf(get_romanz_electrs_path().unwrap(), &node, &config)
            .unwrap_err();
    assert!(matches!(error, Error::StartupAttemptsExhausted(0)));
}

/// Verify directory, spawn, retry, and client-timeout startup failures.
#[cfg(unix)]
#[test]
fn romanz_electrsd_reports_test_program_startup_failures() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    node.write_cookie("user:password");

    let (_program_directory, program) = test_program("exit 1", true);
    let config = RomanzElectrsDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..RomanzElectrsDConf::default()
    };
    assert!(matches!(
        RomanzElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::Io(_))
    ));

    let (_program_directory, program) = test_program("exit 1", false);
    let config = RomanzElectrsDConf {
        max_retries: 1,
        ..RomanzElectrsDConf::default()
    };
    assert!(matches!(
        RomanzElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_directory, program) = test_program("exit 1", true);
    let config = RomanzElectrsDConf {
        max_retries: 2,
        ..RomanzElectrsDConf::default()
    };
    assert!(matches!(
        RomanzElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    let (_program_directory, program) = test_program("exec sleep 30", true);
    let config = RomanzElectrsDConf {
        max_retries: 1,
        ..RomanzElectrsDConf::default()
    };
    assert!(matches!(
        RomanzElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));
}

/// Verify pruned backing nodes are rejected before startup.
#[test]
fn romanz_electrsd_rejects_pruned_backends() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }))
        .with_prune(PruneMode::Automatic(1));
    node.write_cookie("user:password");

    assert!(matches!(
        RomanzElectrsD::from_bin(get_romanz_electrs_path().unwrap(), &node),
        Err(Error::Indexer(IndexerError::InvalidConfiguration(_)))
    ));
}

/// Verify Electrum history transport and protocol failures are classified.
#[test]
fn romanz_electrsd_classifies_history_failures() {
    let script = ScriptBuf::new();
    let txid = Txid::all_zeros();

    let (client, server) = scripted_electrum_client(None);
    assert!(!RomanzElectrsD::script_history_has_mempool_tx(&client, &script, txid).unwrap());
    server.join().unwrap();

    let (client, server) = scripted_electrum_client(Some(Err(serde_json::json!({
        "code": 1,
        "message": "unavailable"
    }))));
    assert!(matches!(
        RomanzElectrsD::script_history_has_mempool_tx(&client, &script, txid),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();
}

/// Verify client setup distinguishes an exited process from an unavailable socket.
#[cfg(unix)]
#[test]
fn romanz_electrsd_reports_client_setup_failures() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    drop(listener);

    let mut process = Command::new("sleep").arg("2").spawn().unwrap();
    assert!(matches!(
        RomanzElectrsD::wait_for_client(socket, &mut process, Duration::from_millis(250)),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    process.kill().unwrap();
    process.wait().unwrap();

    let (socket, server) = scripted_electrum_socket(vec![Some(Err(serde_json::json!({
        "code": -1,
        "message": "indexing"
    })))]);
    let mut process = Command::new("sleep").arg("2").spawn().unwrap();
    assert!(matches!(
        RomanzElectrsD::wait_for_client(socket, &mut process, Duration::from_millis(250)),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    process.kill().unwrap();
    process.wait().unwrap();
    server.join().unwrap();

    let mut process = Command::new("true").spawn().unwrap();
    process.wait().unwrap();
    assert!(matches!(
        RomanzElectrsD::wait_for_client(socket, &mut process, Duration::from_secs(1)),
        Err(Error::ClientSetupTimeout)
    ));
}

/// Verify that socket timeouts are treated as incomplete reads.
#[test]
fn romanz_electrsd_treats_socket_timeouts_as_incomplete_reads() {
    for kind in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::UnexpectedEof,
        ErrorKind::BrokenPipe,
    ] {
        let error = ElectrumError::IOError(IoError::from(kind));
        assert!(is_incomplete_read(&error));

        let error = ElectrumError::SharedIOError(Arc::new(IoError::from(kind)));
        assert!(is_incomplete_read(&error));
    }

    assert!(!is_incomplete_read(&ElectrumError::IOError(IoError::from(
        ErrorKind::PermissionDenied
    ))));
    assert!(!is_incomplete_read(&ElectrumError::SharedIOError(
        Arc::new(IoError::from(ErrorKind::PermissionDenied))
    )));

    let error = ElectrumError::Message("complete error".to_string());
    assert!(!is_incomplete_read(&error));

    assert!(matches!(
        unresponsive_indexer(error),
        IndexerError::UnresponsiveIndexer {
            indexer: "RomanzElectrsD",
            ..
        }
    ));
}

/// Verify `romanz/electrs` header matching without a running server.
#[test]
fn romanz_electrsd_matches_header_notifications() {
    let header = genesis_block(Network::Regtest).header;
    let notification = HeaderNotification { height: 1, header };

    let matched = electrs_header_matches_with(&notification, 2, None, |_| {
        unreachable!("an older notification does not request a header")
    })
    .unwrap();
    assert!(!matched);

    let matched = electrs_header_matches_with(
        &notification,
        1,
        Some(notification.header.block_hash()),
        |_| unreachable!("an equal-height notification supplies the header"),
    )
    .unwrap();
    assert!(matched);

    let matched = electrs_header_matches_with(&notification, 0, None, |height| {
        assert_eq!(height, 0);
        Ok(header)
    })
    .unwrap();
    assert!(matched);

    let incomplete = ElectrumError::IOError(IoError::from(ErrorKind::UnexpectedEof));
    let matched = electrs_header_matches_with(&notification, 0, None, |_| Err(incomplete)).unwrap();
    assert!(!matched);

    let error = electrs_header_matches_with(&notification, 0, None, |_| {
        Err(ElectrumError::Message("unavailable".to_string()))
    })
    .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::UnresponsiveIndexer { .. })
    ));

    let matched = electrs_header_matches_with(
        &notification,
        1,
        Some(genesis_block(Network::Bitcoin).block_hash()),
        |_| unreachable!("an equal-height notification supplies the header"),
    )
    .unwrap();
    assert!(!matched);
}

/// Verify oversized notification heights are rejected.
#[cfg(target_pointer_width = "64")]
#[test]
fn romanz_electrsd_rejects_oversized_notification_height() {
    let header = genesis_block(Network::Regtest).header;
    let notification = HeaderNotification {
        height: usize::MAX,
        header,
    };

    let error = electrs_header_matches_with(&notification, 0, None, |_| Ok(header)).unwrap_err();
    assert!(matches!(error, Error::UnexpectedResponse(_)));
}

/// Verify that [`RomanzElectrsD`] accepts requests without a Bitcoin Core P2P connection.
#[cfg(feature = "bitcoind")]
#[test]
#[allow(clippy::too_many_lines)]
fn romanz_electrsd_accepts_bitcoind() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);

    let mut romanz_electrsd = RomanzElectrsD::new(&bitcoind).unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();
    romanz_electrsd
        .wait_until_block(height, None, Some(INDEXING_TIMEOUT))
        .unwrap();
    let tip = romanz_electrsd.client.block_headers_subscribe().unwrap();
    let notification = HeaderNotification {
        height: tip.height + 1,
        header: tip.header,
    };
    assert!(
        electrs_header_matches(
            &romanz_electrsd.client,
            &notification,
            u32::try_from(tip.height).unwrap(),
            Some(tip.header.block_hash()),
        )
        .unwrap()
    );
    let error = romanz_electrsd
        .wait_until_tip(height + 1, block_hash, Some(Duration::ZERO))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));
    assert!(matches!(
        RomanzElectrsD::wait_for_client(
            romanz_electrsd.get_electrum_socket(),
            &mut romanz_electrsd.process,
            Duration::ZERO,
        ),
        Err(Error::ClientSetupTimeout)
    ));

    romanz_electrsd.client.ping().unwrap();

    info!("PID: {}", romanz_electrsd.get_pid());
    info!(
        "Working Directory: {:?}",
        romanz_electrsd.get_working_directory()
    );
    info!("Electrum Socket: {}", romanz_electrsd.get_electrum_socket());
    info!(
        "Electrum Server Protocol Version: {}",
        romanz_electrsd
            .client
            .server_features()
            .unwrap()
            .protocol_max
    );
    info!(
        "Monitoring Socket: {}",
        romanz_electrsd.get_monitoring_socket()
    );

    let notification = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let protocol_error = serde_json::json!({ "code": 1, "message": "unavailable" });
    let (socket, server) = scripted_electrum_socket(vec![
        Some(Ok(notification.clone())),
        Some(Err(protocol_error)),
    ]);
    romanz_electrsd.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        romanz_electrsd.wait_until_block(height, None, Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();

    let (socket, release, server) = stalled_electrum_socket(notification);
    romanz_electrsd.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    let result = romanz_electrsd.wait_until_block(height, None, Some(Duration::from_millis(250)));
    release.send(()).unwrap();
    server.join().unwrap();
    assert!(matches!(
        result,
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));

    let older_notification = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let (socket, server) = scripted_electrum_socket(vec![
        Some(Ok(older_notification)),
        Some(Ok(Value::Null)),
        Some(Ok(Value::Null)),
    ]);
    romanz_electrsd.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        romanz_electrsd.wait_until_block(height + 1, None, Some(Duration::from_millis(250))),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    server.join().unwrap();

    let initial_header = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let (socket, server) = electrum_server_with_invalid_queued_header(initial_header, height);
    romanz_electrsd.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        romanz_electrsd.wait_until_block(height + 1, None, Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();
}

/// Verify that rejection of [`BtcD`] occurs before data directory creation.
#[cfg(feature = "btcd")]
#[test]
fn romanz_electrsd_rejects_btcd() {
    let btcd = BtcD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = RomanzElectrsDConf {
        staticdir: Some(directory.clone()),
        ..RomanzElectrsDConf::default()
    };

    assert!(matches!(
        RomanzElectrsD::new_with_conf(&btcd, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "BtcD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that rejection of [`UtreexoD`] occurs before data directory creation.
#[cfg(feature = "utreexod")]
#[test]
fn romanz_electrsd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = RomanzElectrsDConf {
        staticdir: Some(directory.clone()),
        ..RomanzElectrsDConf::default()
    };

    assert!(matches!(
        RomanzElectrsD::new_with_conf(&utreexod, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that rejection of [`FlorestaD`] occurs before data directory creation.
#[cfg(feature = "florestad")]
#[test]
fn romanz_electrsd_rejects_florestad() {
    let florestad = FlorestaD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = RomanzElectrsDConf {
        staticdir: Some(directory.clone()),
        ..RomanzElectrsDConf::default()
    };

    assert!(matches!(
        RomanzElectrsD::new_with_conf(&florestad, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "FlorestaD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that [`RomanzElectrsD`] indexes mempool transactions.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_sees_mempool_transactions() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let romanz_electrsd = RomanzElectrsD::new(&bitcoind).unwrap();

    romanz_electrsd.client.ping().unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();

    let address = bitcoind
        .client
        .get_new_address(None, None)
        .unwrap()
        .address()
        .unwrap()
        .assume_checked();
    let script_pubkey = address.script_pubkey();
    let error = romanz_electrsd
        .wait_until_mempool_tx(
            &script_pubkey,
            Txid::all_zeros(),
            Some(Duration::from_millis(1)),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));
    let txid = bitcoind
        .client
        .send_to_address(&address, Amount::from_int_btc(1))
        .unwrap()
        .txid()
        .unwrap();
    romanz_electrsd.trigger().unwrap();

    let error = romanz_electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(Duration::ZERO))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));

    romanz_electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(INDEXING_TIMEOUT))
        .unwrap();
}

/// Verify repeated synchronization of [`RomanzElectrsD`] with the [`BitcoinD`] chain tip.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_syncs_blocks() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(SYNC_INITIAL_BLOCK_COUNT).unwrap();

    let romanz_electrsd = RomanzElectrsD::new(&bitcoind).unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();

    let mut height = SYNC_INITIAL_BLOCK_COUNT;
    for count in SYNC_BLOCK_BATCHES {
        bitcoind.generate(*count).unwrap();
        romanz_electrsd
            .wait_until_caught_up(&bitcoind, None)
            .unwrap();

        height += count;
        let block_hash = bitcoind.get_block_hash(height).unwrap();
        romanz_electrsd
            .wait_until_tip(height, block_hash, Some(INDEXING_TIMEOUT))
            .unwrap();
        romanz_electrsd
            .wait_until_caught_up(&bitcoind, None)
            .unwrap();
    }
}

/// Verify that [`RomanzElectrsD`] uses the replacement tip after a reorganization.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_reindexes_reorgs() {
    let bitcoind = BitcoinD::new().unwrap();
    let romanz_electrsd = RomanzElectrsD::new(&bitcoind).unwrap();

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();

    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let tip = romanz_electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, height);
    assert_eq!(tip.header.block_hash(), block_hash);

    bitcoind.invalidate_blocks(1).unwrap();
    bitcoind.generate(1).unwrap();

    let replacement_height = bitcoind.get_chain_tip().unwrap();
    let replacement_hash = bitcoind.get_block_hash(replacement_height).unwrap();

    assert_ne!(block_hash, replacement_hash);
    assert_eq!(height, replacement_height);

    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let tip = romanz_electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, replacement_height);
    assert_eq!(tip.header.block_hash(), replacement_hash);
}

#[test]
fn romanz_electrsd_configuration_defaults() {
    let config = RomanzElectrsDConf::default();

    assert!(config.raw_args.is_empty());
    assert_eq!(config.max_retries, SPAWN_ATTEMPTS);
    assert_eq!(
        RomanzElectrsD::configured_args(&config, Network::Regtest).unwrap(),
        ["--network", "regtest"]
    );
}

#[test]
fn romanz_electrsd_renders_every_network() {
    let cases = [
        (Network::Bitcoin, "bitcoin"),
        (Network::Testnet, "testnet"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ];

    for (network, expected) in cases {
        let config = RomanzElectrsDConf::default();

        assert_eq!(
            RomanzElectrsD::configured_args(&config, network).unwrap(),
            ["--network", expected]
        );
    }
}

#[test]
fn romanz_electrsd_rejects_owned_raw_arguments() {
    let cases = [
        "--network",
        "--network=signet",
        "--db-dir",
        "--db-dir=/tmp/electrs",
        "--daemon-rpc-addr",
        "--daemon-rpc-addr=127.0.0.1:1",
        "--electrum-rpc-addr",
        "--electrum-rpc-addr=127.0.0.1:3",
        "--monitoring-addr",
        "--monitoring-addr=127.0.0.1:4",
        "--cookie-file",
        "--cookie-file=/tmp/.cookie",
    ];

    for arg in cases {
        let config = RomanzElectrsDConf {
            raw_args: vec![arg.to_string()],
            ..RomanzElectrsDConf::default()
        };

        assert!(matches!(
            RomanzElectrsD::configured_args(&config, Network::Regtest),
            Err(Error::Indexer(IndexerError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }
}

#[test]
fn romanz_electrsd_accepts_unmodeled_raw_arguments() {
    let config = RomanzElectrsDConf {
        raw_args: vec![
            "--log-filters=debug".to_string(),
            "--index-batch-size=100".to_string(),
        ],
        ..RomanzElectrsDConf::default()
    };

    assert!(RomanzElectrsD::configured_args(&config, Network::Regtest).is_ok());
}

/// Verify process state, shutdown, and temporary cleanup.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let bitcoind = BitcoinD::new().unwrap();
    let config = RomanzElectrsDConf::default();
    let mut romanz_electrsd = RomanzElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    let directory = romanz_electrsd.get_working_directory();

    assert!(romanz_electrsd.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(romanz_electrsd.get_config(), &config);
    assert!(romanz_electrsd.get_electrum_socket().ip().is_loopback());
    assert!(romanz_electrsd.get_monitoring_socket().ip().is_loopback());
    assert_ne!(
        romanz_electrsd.get_electrum_socket(),
        romanz_electrsd.get_monitoring_socket()
    );
    romanz_electrsd.client.ping().unwrap();

    romanz_electrsd.stop().unwrap();
    #[cfg(not(target_os = "windows"))]
    assert!(matches!(
        romanz_electrsd.trigger(),
        Err(Error::UnexpectedResponse(_))
    ));
    #[cfg(target_os = "windows")]
    romanz_electrsd.trigger().unwrap();
    drop(romanz_electrsd);
    assert!(!directory.exists());
}

/// Verify confirmed and unconfirmed balances across a reorganization.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_updates_balances_across_reorganizations() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let romanz_electrsd = RomanzElectrsD::new(&bitcoind).unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();

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
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let balance = romanz_electrsd
        .client
        .script_get_balance(&script_pubkey)
        .unwrap();
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

    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    romanz_electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, Some(INDEXING_TIMEOUT))
        .unwrap();
    let balance = romanz_electrsd
        .client
        .script_get_balance(&script_pubkey)
        .unwrap();
    assert_eq!(balance.confirmed, 0);
    assert_eq!(balance.unconfirmed, i64::try_from(amount.to_sat()).unwrap());

    bitcoind.generate(CONFIRMATION_BLOCK_COUNT).unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let balance = romanz_electrsd
        .client
        .script_get_balance(&script_pubkey)
        .unwrap();
    assert_eq!(balance.confirmed, amount.to_sat());
    assert_eq!(balance.unconfirmed, 0);
}

/// Verify that a static directory retains indexed chain state across a restart.
#[cfg(feature = "bitcoind")]
#[test]
fn romanz_electrsd_static_directory_restores_indexed_state() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(PERSISTENCE_BLOCK_COUNT).unwrap();

    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("electrs");
    let config = RomanzElectrsDConf {
        staticdir: Some(directory.clone()),
        ..RomanzElectrsDConf::default()
    };

    let mut romanz_electrsd = RomanzElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    romanz_electrsd.stop().unwrap();
    drop(romanz_electrsd);

    assert!(directory.is_dir());

    let mut romanz_electrsd = RomanzElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    romanz_electrsd
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let tip = romanz_electrsd.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        tip.header.block_hash(),
        bitcoind.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap()
    );
    romanz_electrsd.stop().unwrap();
    drop(romanz_electrsd);
    assert!(directory.is_dir());
}
