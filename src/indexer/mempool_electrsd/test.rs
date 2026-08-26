// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`MempoolElectrsD`].

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
use std::time::Instant;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::BlockHash;
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
use tracing::Level;

#[cfg(feature = "bitcoind")]
use super::MEMPOOL_ELECTRS_INDEXING_TIMEOUT;
use super::MempoolElectrsD;
use super::MempoolElectrsDConf;
use super::get_mempool_electrs_path;
use super::is_incomplete_read;
#[cfg(feature = "bitcoind")]
use super::mempool_electrs_header_matches;
use super::mempool_electrs_header_matches_with;
use super::unresponsive_indexer;
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
use crate::indexer::test::FakeNode;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::read_scripted_electrum_request;
use crate::indexer::test::scripted_electrum_client;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::scripted_electrum_reader;
#[cfg(feature = "bitcoind")]
use crate::indexer::test::scripted_electrum_socket;
#[cfg(unix)]
use crate::indexer::test::test_program;
use crate::node::PruneMode;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Start an Electrum server that queues an invalid raw header during two ping calls.
#[cfg(feature = "bitcoind")]
fn electrum_server_with_invalid_queued_header(
    initial_header: serde_json::Value,
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

/// Verify the Esplora client and chain-tip endpoints against the backing node.
#[cfg(feature = "bitcoind")]
fn assert_esplora_tip(mempool_electrs: &MempoolElectrsD, bitcoind: &BitcoinD) {
    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();
    let esplora = mempool_electrs.get_esplora_client();

    assert!(mempool_electrs.get_esplora_socket().ip().is_loopback());
    assert_eq!(
        mempool_electrs.get_esplora_url(),
        format!("http://{}", mempool_electrs.get_esplora_socket())
    );
    assert_eq!(esplora.url(), mempool_electrs.get_esplora_url());

    let start = Instant::now();
    while start.elapsed() < MEMPOOL_ELECTRS_INDEXING_TIMEOUT {
        if matches!(esplora.get_height(), Ok(actual) if actual == height)
            && matches!(esplora.get_tip_hash(), Ok(actual) if actual == block_hash)
        {
            break;
        }
        mempool_electrs.trigger().unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }

    assert_eq!(esplora.get_height().unwrap(), height);
    assert_eq!(esplora.get_tip_hash().unwrap(), block_hash);
    assert_eq!(esplora.get_block_hash(height).unwrap(), block_hash);
    assert_eq!(
        esplora
            .get_header_by_hash(&block_hash)
            .unwrap()
            .block_hash(),
        block_hash
    );
}

/// Poll until Esplora reports the expected transaction confirmation state.
#[cfg(feature = "bitcoind")]
fn assert_esplora_transaction_status(
    mempool_electrs: &MempoolElectrsD,
    txid: Txid,
    confirmed: bool,
    block_hash: Option<BlockHash>,
) {
    let esplora = mempool_electrs.get_esplora_client();
    let start = Instant::now();
    while start.elapsed() < MEMPOOL_ELECTRS_INDEXING_TIMEOUT {
        if esplora
            .get_tx_status(&txid)
            .is_ok_and(|status| status.confirmed == confirmed && status.block_hash == block_hash)
        {
            return;
        }
        mempool_electrs.trigger().unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }

    let status = esplora.get_tx_status(&txid).unwrap();
    assert_eq!(status.confirmed, confirmed);
    assert_eq!(status.block_hash, block_hash);
}

/// Verify binary-path validation and zero start attempts.
#[test]
fn mempool_electrsd_validates_binary_path_and_start_attempts() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));

    let error = MempoolElectrsD::from_bin("electrs", &node).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotAbsolute { .. }));

    let root = tempfile::tempdir().unwrap();
    let error = MempoolElectrsD::from_bin(root.path().join("missing-electrs"), &node).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotFile { .. }));

    node.write_cookie("user:password");
    let config = MempoolElectrsDConf {
        max_retries: 0,
        ..MempoolElectrsDConf::default()
    };
    let error =
        MempoolElectrsD::from_bin_with_conf(get_mempool_electrs_path().unwrap(), &node, &config)
            .unwrap_err();
    assert!(matches!(error, Error::StartupAttemptsExhausted(0)));
}

/// Verify directory, spawn, retry, and client-timeout startup failures.
#[cfg(unix)]
#[test]
fn mempool_electrsd_reports_test_program_startup_failures() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    node.write_cookie("user:password");

    let (_program_directory, program) = test_program("exit 1", true);
    let config = MempoolElectrsDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..MempoolElectrsDConf::default()
    };
    assert!(matches!(
        MempoolElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::Io(_))
    ));

    let (_program_directory, program) = test_program("exit 1", false);
    let config = MempoolElectrsDConf {
        max_retries: 1,
        ..MempoolElectrsDConf::default()
    };
    assert!(matches!(
        MempoolElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_directory, program) = test_program("exit 1", true);
    let config = MempoolElectrsDConf {
        max_retries: 2,
        ..MempoolElectrsDConf::default()
    };
    assert!(matches!(
        MempoolElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    let (_program_directory, program) = test_program("exec sleep 30", true);
    let config = MempoolElectrsDConf {
        max_retries: 1,
        ..MempoolElectrsDConf::default()
    };
    assert!(matches!(
        MempoolElectrsD::from_bin_with_conf(&program, &node, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));
}

/// Verify pruned backing nodes are rejected before startup.
#[test]
fn mempool_electrsd_rejects_pruned_backends() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }))
        .with_prune(PruneMode::Automatic(1));
    node.write_cookie("user:password");

    assert!(matches!(
        MempoolElectrsD::from_bin(get_mempool_electrs_path().unwrap(), &node),
        Err(Error::Indexer(IndexerError::InvalidConfiguration(_)))
    ));
}

/// Verify Electrum history transport and protocol failures are classified.
#[test]
fn mempool_electrsd_classifies_history_failures() {
    let script = ScriptBuf::new();
    let txid = Txid::all_zeros();

    let (client, server) = scripted_electrum_client(None);
    assert!(!MempoolElectrsD::script_history_has_mempool_tx(&client, &script, txid).unwrap());
    server.join().unwrap();

    let (client, server) = scripted_electrum_client(Some(Err(serde_json::json!({
        "code": 1,
        "message": "unavailable"
    }))));
    assert!(matches!(
        MempoolElectrsD::script_history_has_mempool_tx(&client, &script, txid),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();
}

/// Verify client setup distinguishes an exited process from an unavailable socket.
#[cfg(unix)]
#[test]
fn mempool_electrsd_reports_client_setup_failures() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    drop(listener);

    let mut process = Command::new("sleep").arg("2").spawn().unwrap();
    assert!(matches!(
        MempoolElectrsD::wait_for_client(socket, &mut process, Duration::from_millis(250)),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    process.kill().unwrap();
    process.wait().unwrap();

    let mut process = Command::new("true").spawn().unwrap();
    process.wait().unwrap();
    assert!(matches!(
        MempoolElectrsD::wait_for_client(socket, &mut process, Duration::from_secs(1)),
        Err(Error::ClientSetupTimeout)
    ));
}

/// Verify socket timeouts are treated as incomplete reads.
#[test]
fn mempool_electrsd_treats_socket_timeouts_as_incomplete_reads() {
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

    let error = ElectrumError::Message("complete error".to_string());
    assert!(!is_incomplete_read(&error));
    assert!(matches!(
        unresponsive_indexer(error),
        IndexerError::UnresponsiveIndexer {
            indexer: "MempoolElectrsD",
            ..
        }
    ));
}

/// Verify `mempool/electrs` header matching without a running server.
#[test]
fn mempool_electrsd_matches_header_notifications() {
    let header = genesis_block(Network::Regtest).header;
    let notification = HeaderNotification { height: 1, header };

    assert!(
        !mempool_electrs_header_matches_with(&notification, 2, None, |_| {
            unreachable!("an older notification does not request a header")
        })
        .unwrap()
    );

    assert!(
        mempool_electrs_header_matches_with(
            &notification,
            1,
            Some(notification.header.block_hash()),
            |_| unreachable!("an equal-height notification supplies the header"),
        )
        .unwrap()
    );

    assert!(
        mempool_electrs_header_matches_with(&notification, 0, None, |height| {
            assert_eq!(height, 0);
            Ok(header)
        })
        .unwrap()
    );

    let incomplete = ElectrumError::IOError(IoError::from(ErrorKind::UnexpectedEof));
    assert!(
        !mempool_electrs_header_matches_with(&notification, 0, None, |_| Err(incomplete)).unwrap()
    );

    let error = mempool_electrs_header_matches_with(&notification, 0, None, |_| {
        Err(ElectrumError::Message("unavailable".to_string()))
    })
    .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::UnresponsiveIndexer { .. })
    ));

    assert!(
        !mempool_electrs_header_matches_with(
            &notification,
            1,
            Some(genesis_block(Network::Bitcoin).block_hash()),
            |_| unreachable!("an equal-height notification supplies the header"),
        )
        .unwrap()
    );
}

/// Verify oversized notification heights are rejected.
#[cfg(target_pointer_width = "64")]
#[test]
fn mempool_electrsd_rejects_oversized_notification_height() {
    let header = genesis_block(Network::Regtest).header;
    let notification = HeaderNotification {
        height: usize::MAX,
        header,
    };

    let error =
        mempool_electrs_header_matches_with(&notification, 0, None, |_| Ok(header)).unwrap_err();
    assert!(matches!(error, Error::UnexpectedResponse(_)));
}

/// Verify that [`MempoolElectrsD`] accepts Bitcoin Core and serves Electrum and Esplora.
#[cfg(feature = "bitcoind")]
#[test]
#[allow(clippy::too_many_lines)]
fn mempool_electrsd_accepts_bitcoind() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_test_writer()
        .try_init();

    let bitcoind = BitcoinD::new().unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);

    let mut mempool_electrs = MempoolElectrsD::new(&bitcoind).unwrap();
    assert_eq!(bitcoind.get_peer_count().unwrap(), 0);

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();
    mempool_electrs
        .wait_until_block(height, None, Some(MEMPOOL_ELECTRS_INDEXING_TIMEOUT))
        .unwrap();
    assert_esplora_tip(&mempool_electrs, &bitcoind);

    let tip = mempool_electrs.client.block_headers_subscribe().unwrap();
    let notification = HeaderNotification {
        height: tip.height + 1,
        header: tip.header,
    };
    assert!(
        mempool_electrs_header_matches(
            &mempool_electrs.client,
            &notification,
            u32::try_from(tip.height).unwrap(),
            Some(tip.header.block_hash()),
        )
        .unwrap()
    );

    let error = mempool_electrs
        .wait_until_tip(height + 1, block_hash, Some(Duration::ZERO))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));
    assert!(matches!(
        MempoolElectrsD::wait_for_client(
            mempool_electrs.get_electrum_socket(),
            &mut mempool_electrs.process,
            Duration::ZERO,
        ),
        Err(Error::ClientSetupTimeout)
    ));

    mempool_electrs.client.ping().unwrap();
    assert!(mempool_electrs.get_pid() > 0);
    assert!(mempool_electrs.get_working_directory().is_dir());
    assert!(mempool_electrs.get_electrum_socket().ip().is_loopback());
    assert_eq!(
        mempool_electrs.get_electrum_url(),
        mempool_electrs.get_electrum_socket().to_string()
    );
    assert!(mempool_electrs.get_monitoring_socket().ip().is_loopback());

    let notification = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let protocol_error = serde_json::json!({ "code": 1, "message": "unavailable" });
    let (socket, server) = scripted_electrum_socket(vec![
        Some(Ok(notification.clone())),
        Some(Err(protocol_error)),
    ]);
    mempool_electrs.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        mempool_electrs.wait_until_block(height, None, Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();

    let (socket, server) = scripted_electrum_socket(vec![Some(Ok(notification)), None]);
    mempool_electrs.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        mempool_electrs.wait_until_block(height, None, Some(Duration::from_millis(250))),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    server.join().unwrap();

    let older_notification = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let (socket, server) = scripted_electrum_socket(vec![
        Some(Ok(older_notification)),
        Some(Ok(serde_json::Value::Null)),
        Some(Ok(serde_json::Value::Null)),
    ]);
    mempool_electrs.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        mempool_electrs.wait_until_block(height + 1, None, Some(Duration::from_millis(250))),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    server.join().unwrap();

    let initial_header = serde_json::json!({
        "height": height,
        "hex": serialize(&genesis_block(Network::Regtest).header).to_lower_hex_string(),
    });
    let (socket, server) = electrum_server_with_invalid_queued_header(initial_header, height);
    mempool_electrs.client =
        electrum_client::raw_client::RawClient::new(socket, Some(Duration::from_secs(1)), None)
            .unwrap();
    assert!(matches!(
        mempool_electrs.wait_until_block(height + 1, None, Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    server.join().unwrap();
}

/// Verify rejection of [`UtreexoD`] occurs before data directory creation.
#[cfg(feature = "utreexod")]
#[test]
fn mempool_electrsd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("mempool-electrs");
    let config = MempoolElectrsDConf {
        staticdir: Some(directory.clone()),
        ..MempoolElectrsDConf::default()
    };

    assert!(matches!(
        MempoolElectrsD::new_with_conf(&utreexod, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify rejection of [`FlorestaD`] occurs before data directory creation.
#[cfg(feature = "florestad")]
#[test]
fn mempool_electrsd_rejects_florestad() {
    let florestad = FlorestaD::new().unwrap();
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("mempool-electrs");
    let config = MempoolElectrsDConf {
        staticdir: Some(directory.clone()),
        ..MempoolElectrsDConf::default()
    };

    assert!(matches!(
        MempoolElectrsD::new_with_conf(&florestad, &config),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "FlorestaD"
        }))
    ));
    assert!(!directory.exists());
}

/// Verify that Electrum and Esplora both expose mempool transactions.
#[cfg(feature = "bitcoind")]
#[test]
fn mempool_electrsd_sees_mempool_transactions() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    let mempool_electrs = MempoolElectrsD::new(&bitcoind).unwrap();

    mempool_electrs.client.ping().unwrap();
    mempool_electrs
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let error = mempool_electrs
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
    mempool_electrs.trigger().unwrap();
    assert!(matches!(
        mempool_electrs.wait_until_mempool_tx(&script_pubkey, txid, Some(Duration::ZERO)),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    mempool_electrs
        .wait_until_mempool_tx(&script_pubkey, txid, Some(MEMPOOL_ELECTRS_INDEXING_TIMEOUT))
        .unwrap();

    let esplora = mempool_electrs.get_esplora_client();
    assert!(esplora.get_mempool_txids().unwrap().contains(&txid));
    assert_esplora_transaction_status(&mempool_electrs, txid, false, None);
    assert_eq!(esplora.get_tx(&txid).unwrap().unwrap().compute_txid(), txid);
    assert!(
        esplora
            .get_mempool_scripthash_txs(&script_pubkey)
            .unwrap()
            .iter()
            .any(|transaction| transaction.txid == txid)
    );

    let confirmation_hash = bitcoind.generate(1).unwrap()[0];
    mempool_electrs
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    assert_esplora_transaction_status(&mempool_electrs, txid, true, Some(confirmation_hash));
}

/// Verify repeated synchronization through both Electrum and Esplora.
#[cfg(feature = "bitcoind")]
#[test]
fn mempool_electrsd_syncs_blocks() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(SYNC_INITIAL_BLOCK_COUNT).unwrap();

    let mempool_electrs = MempoolElectrsD::new(&bitcoind).unwrap();
    mempool_electrs
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    assert_esplora_tip(&mempool_electrs, &bitcoind);

    let mut height = SYNC_INITIAL_BLOCK_COUNT;
    for count in SYNC_BLOCK_BATCHES {
        bitcoind.generate(*count).unwrap();
        mempool_electrs
            .wait_until_caught_up(&bitcoind, None)
            .unwrap();

        height += count;
        let block_hash = bitcoind.get_block_hash(height).unwrap();
        mempool_electrs
            .wait_until_tip(height, block_hash, Some(MEMPOOL_ELECTRS_INDEXING_TIMEOUT))
            .unwrap();
        assert_esplora_tip(&mempool_electrs, &bitcoind);
    }
}

#[test]
fn mempool_electrsd_configuration_defaults() {
    let config = MempoolElectrsDConf::default();

    assert!(config.raw_args.is_empty());
    assert_eq!(config.max_retries, SPAWN_ATTEMPTS);
    assert_eq!(
        MempoolElectrsD::configured_args(&config, Network::Regtest).unwrap(),
        ["--network", "regtest"]
    );
}

#[test]
fn mempool_electrsd_renders_every_network() {
    let cases = [
        (Network::Bitcoin, "bitcoin"),
        (Network::Testnet, "testnet"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ];

    for (network, expected) in cases {
        assert_eq!(
            MempoolElectrsD::configured_args(&MempoolElectrsDConf::default(), network).unwrap(),
            ["--network", expected]
        );
    }
}

#[test]
fn mempool_electrsd_rejects_owned_raw_arguments() {
    let cases = [
        "--network",
        "--network=signet",
        "--db-dir",
        "--db-dir=/tmp/electrs",
        "--daemon-rpc-addr",
        "--daemon-rpc-addr=127.0.0.1:1",
        "--http-addr",
        "--http-addr=127.0.0.1:2",
        "--electrum-rpc-addr",
        "--electrum-rpc-addr=127.0.0.1:3",
        "--monitoring-addr",
        "--monitoring-addr=127.0.0.1:4",
        "--cookie",
        "--cookie=user:password",
        "--jsonrpc-import",
    ];

    for arg in cases {
        let config = MempoolElectrsDConf {
            raw_args: vec![arg.to_string()],
            ..MempoolElectrsDConf::default()
        };

        assert!(matches!(
            MempoolElectrsD::configured_args(&config, Network::Regtest),
            Err(Error::Indexer(IndexerError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }
}

#[test]
fn mempool_electrsd_accepts_unmodeled_raw_arguments() {
    let config = MempoolElectrsDConf {
        raw_args: vec![
            "--log-filters=debug".to_string(),
            "--index-batch-size=100".to_string(),
        ],
        ..MempoolElectrsDConf::default()
    };

    assert!(MempoolElectrsD::configured_args(&config, Network::Regtest).is_ok());
}

/// Verify process state, sockets, shutdown, and temporary cleanup.
#[cfg(feature = "bitcoind")]
#[test]
fn mempool_electrsd_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let bitcoind = BitcoinD::new().unwrap();
    let config = MempoolElectrsDConf::default();
    let mut mempool_electrs = MempoolElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    let directory = mempool_electrs.get_working_directory();

    assert!(mempool_electrs.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(mempool_electrs.get_config(), &config);
    assert!(mempool_electrs.get_electrum_socket().ip().is_loopback());
    assert!(mempool_electrs.get_monitoring_socket().ip().is_loopback());
    assert!(mempool_electrs.get_esplora_socket().ip().is_loopback());
    assert_ne!(
        mempool_electrs.get_electrum_socket(),
        mempool_electrs.get_monitoring_socket()
    );
    assert_ne!(
        mempool_electrs.get_electrum_socket(),
        mempool_electrs.get_esplora_socket()
    );
    assert_esplora_tip(&mempool_electrs, &bitcoind);

    mempool_electrs.stop().unwrap();
    assert!(matches!(
        mempool_electrs.trigger(),
        Err(Error::UnexpectedResponse(_))
    ));
    drop(mempool_electrs);
    assert!(!directory.exists());
}

/// Verify a static directory retains indexed state for Electrum and Esplora.
#[cfg(feature = "bitcoind")]
#[test]
fn mempool_electrsd_static_directory_restores_indexed_state() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(PERSISTENCE_BLOCK_COUNT).unwrap();

    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("mempool-electrs");
    let config = MempoolElectrsDConf {
        staticdir: Some(directory.clone()),
        ..MempoolElectrsDConf::default()
    };

    let mut mempool_electrs = MempoolElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    mempool_electrs
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    assert_esplora_tip(&mempool_electrs, &bitcoind);
    mempool_electrs.stop().unwrap();
    drop(mempool_electrs);
    assert!(directory.is_dir());

    let mut mempool_electrs = MempoolElectrsD::new_with_conf(&bitcoind, &config).unwrap();
    mempool_electrs
        .wait_until_caught_up(&bitcoind, None)
        .unwrap();
    let tip = mempool_electrs.client.block_headers_subscribe().unwrap();
    assert_eq!(tip.height as u32, PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        tip.header.block_hash(),
        bitcoind.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap()
    );
    assert_esplora_tip(&mempool_electrs, &bitcoind);
    mempool_electrs.stop().unwrap();
    drop(mempool_electrs);
    assert!(directory.is_dir());
}
