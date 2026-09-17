// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration, scripted protocol, and process tests for [`LibbitcoinD`].

use core::net::SocketAddr;
use core::time::Duration;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::process::Command;
use std::process::Stdio;
use std::thread::JoinHandle;
use std::thread::sleep;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use corepc_client::bitcoin::ScriptBuf;
use corepc_client::bitcoin::Txid;
use corepc_client::bitcoin::consensus::serialize;
use corepc_client::bitcoin::constants::genesis_block;
use corepc_client::bitcoin::hashes::Hash;
use corepc_client::bitcoin::hex::DisplayHex;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v30::Client;
use electrum_client::ElectrumApi;
use electrum_client::raw_client::RawClient;
use serde_json::Value;
use serde_json::json;
use tracing::Level;

use super::LibbitcoinD;
use super::LibbitcoinDConf;
use super::get_libbitcoin_path;
use crate::DataDir;
use crate::Error;
use crate::SPAWN_ATTEMPTS;
use crate::indexer::Indexer;
use crate::indexer::IndexerError;
use crate::indexer::test::scripted_electrum_socket;
use crate::indexer::validate_backend;
use crate::node::Node;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::node::test::scripted_json_rpc_server;
use crate::node::test::test_program;

/// Build an instance with local scripted RPC and Electrum endpoints.
fn scripted_instance(
    rpc_results: Vec<Value>,
    electrum_results: Vec<Option<Result<Value, Value>>>,
) -> (LibbitcoinD, JoinHandle<()>, JoinHandle<()>) {
    let (rpc_socket, rpc_server) = scripted_json_rpc_server(rpc_results);
    let (electrum_socket, electrum_server) = scripted_electrum_socket(electrum_results);
    let client = Client::new_with_auth(
        &format!("http://{rpc_socket}"),
        Auth::UserPass("user".to_string(), "password".to_string()),
    )
    .unwrap();
    let electrum_client =
        RawClient::new(electrum_socket, Some(Duration::from_secs(1)), None).unwrap();
    let esplora_socket = SocketAddr::from(([127, 0, 0, 1], 1));
    let esplora_client =
        esplora_client::Builder::new(&format!("http://{esplora_socket}")).build_blocking();
    let process = Command::new("/bin/sh")
        .args(["-c", "read line"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    (
        LibbitcoinD {
            process,
            client,
            electrum_client,
            esplora_client,
            working_directory: DataDir::Temporary(tempfile::tempdir().unwrap()),
            config: LibbitcoinDConf::default(),
            p2p_socket: SocketAddr::from(([127, 0, 0, 1], 2)),
            rpc_socket,
            electrum_socket,
            esplora_socket,
        },
        rpc_server,
        electrum_server,
    )
}

#[test]
fn libbitcoin_binary_and_startup_attempts() {
    let bin = get_libbitcoin_path().unwrap();
    assert!(bin.is_file());
    assert!(matches!(
        LibbitcoinD::from_bin("bs"),
        Err(Error::BinaryPathNotAbsolute { .. })
    ));
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        LibbitcoinD::from_bin(dir.path().join("missing-bs")),
        Err(Error::BinaryPathNotFile { .. })
    ));
    let conf = LibbitcoinDConf {
        max_retries: 0,
        ..LibbitcoinDConf::default()
    };
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(bin, &conf),
        Err(Error::StartupAttemptsExhausted(0))
    ));
    assert!(matches!(
        LibbitcoinD::new_with_conf(&conf),
        Err(Error::StartupAttemptsExhausted(0))
    ));
}

#[test]
fn libbitcoin_configuration_defaults_and_trait_names() {
    let conf = LibbitcoinDConf::default();
    assert_eq!(conf.args.network, Network::Bitcoin);
    assert!(conf.args.txindex);
    assert!(conf.raw_args.is_empty());
    assert_eq!(conf.max_retries, SPAWN_ATTEMPTS);
    assert_eq!(conf.as_ref(), &conf.args);
    assert_eq!(<LibbitcoinD as Node>::get_name(), LibbitcoinD::get_name());
    assert_eq!(
        <LibbitcoinD as Indexer>::get_name(),
        LibbitcoinD::get_name()
    );
    assert_eq!(<LibbitcoinD as Node>::get_bin_name(), "bs");
    assert_eq!(<LibbitcoinD as Indexer>::get_bin_name(), "bs");
}

#[test]
fn libbitcoin_cannot_back_a_separate_indexer() {
    assert!(matches!(
        validate_backend::<LibbitcoinD>(),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "LibbitcoinD"
        }))
    ));
}

/// Manually check mainnet genesis across the three interfaces and wait for
/// at least `100_000` headers from a public peer.
#[test]
#[ignore = "starts a mainnet node and synchronizes aproximately 100_000 headers"]
fn libbitcoin_mainnet_smoke_test() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();

    let server = LibbitcoinD::new().unwrap();
    let genesis_hash = genesis_block(Network::Bitcoin).block_hash();

    assert_eq!(server.get_block_hash(0).unwrap(), genesis_hash);
    assert_eq!(
        server
            .get_electrum_client()
            .block_header(0)
            .unwrap()
            .block_hash(),
        genesis_hash
    );
    assert_eq!(
        server.get_esplora_client().get_block_hash(0).unwrap(),
        genesis_hash
    );
    server.get_chain_tip().unwrap();
    server.get_esplora_client().get_height().unwrap();

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        let info = server.call("getblockchaininfo", &[]).unwrap();
        let headers = info["headers"]
            .as_u64()
            .expect("getblockchaininfo must return a numeric header height");
        if headers > 100_000 {
            return;
        }
        sleep(Duration::from_secs(1));
    }
    panic!("libbitcoin did not download a mainnet header within 60 seconds");
}

#[test]
fn libbitcoin_rejects_every_unsupported_node_option() {
    let bin = get_libbitcoin_path().unwrap();
    let default = LibbitcoinDConf::default();
    for network in [
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ] {
        let mut conf = default.clone();
        conf.args.network = network;
        assert!(matches!(
            LibbitcoinD::from_bin_with_conf(&bin, &conf),
            Err(Error::Node(NodeError::InvalidConfiguration(_)))
        ));
    }
    for change in [
        |conf: &mut LibbitcoinDConf| {
            conf.args
                .fixed_peers
                .push(SocketAddr::from(([127, 0, 0, 1], 8333)));
        },
        |conf: &mut LibbitcoinDConf| conf.args.v2_transport = true,
        |conf: &mut LibbitcoinDConf| conf.args.cbf_index = true,
        |conf: &mut LibbitcoinDConf| conf.args.prune = PruneMode::Manual,
        |conf: &mut LibbitcoinDConf| conf.args.prune = PruneMode::Automatic(550),
        |conf: &mut LibbitcoinDConf| conf.args.txindex = false,
    ] {
        let mut conf = default.clone();
        change(&mut conf);
        assert!(matches!(
            LibbitcoinD::from_bin_with_conf(&bin, &conf),
            Err(Error::Node(NodeError::InvalidConfiguration(_)))
        ));
    }
    for raw in ["--config=other.cfg", "-cother.cfg", "--CONFIG", "-c"] {
        let mut conf = default.clone();
        conf.raw_args.push(raw.to_string());
        assert!(matches!(
            LibbitcoinD::from_bin_with_conf(&bin, &conf),
            Err(Error::Node(NodeError::ConflictingArgument(_)))
        ));
    }
}

#[test]
fn libbitcoin_writes_private_config_and_validates_directories() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("data");
    let config = root.path().join("bs.cfg");
    let p2p = SocketAddr::from(([127, 0, 0, 1], 1001));
    let rpc = SocketAddr::from(([127, 0, 0, 1], 1002));
    let electrum = SocketAddr::from(([127, 0, 0, 1], 1003));
    let esplora = SocketAddr::from(([127, 0, 0, 1], 1004));
    LibbitcoinD::write_config(&config, &dir, p2p, rpc, electrum, esplora).unwrap();
    let content = fs::read_to_string(&config).unwrap();
    for expected in [
        format!("[database]\npath = {}", dir.join("database").display()),
        format!("[peer]\npath = {}", dir.join("network").display()),
        format!("[log]\npath = {}", dir.join("log").display()),
        "[node]\ndelay_inbound = false".to_string(),
        format!("bind = {p2p}"),
        format!("bind = {rpc}"),
        format!("bind = {electrum}"),
        format!("bind = {esplora}"),
        "credential = __cookie__:halfin".to_string(),
    ] {
        assert!(content.contains(&expected), "missing {expected}");
    }
    let parsed = Command::new(get_libbitcoin_path().unwrap())
        .args(["--config", config.to_str().unwrap(), "--settings"])
        .output()
        .unwrap();
    assert!(
        parsed.status.success(),
        "bs rejected generated config: {}",
        String::from_utf8_lossy(&parsed.stderr)
    );
    let bin = get_libbitcoin_path().unwrap();
    let conf = LibbitcoinDConf {
        tmpdir: Some(root.path().to_path_buf()),
        staticdir: Some(dir),
        ..LibbitcoinDConf::default()
    };
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(bin, &conf),
        Err(Error::BothDirsSpecified)
    ));
}

#[test]
fn libbitcoin_reports_spawn_and_immediate_exit_failures() {
    let (_directory, program) = test_program("exit 1", false);
    let conf = LibbitcoinDConf {
        max_retries: 1,
        ..LibbitcoinDConf::default()
    };
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(&program, &conf),
        Err(Error::FailedToSpawn(_))
    ));
    let (_directory, program) = test_program("exit 1", true);
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(&program, &conf),
        Err(Error::StartupAttemptsExhausted(1))
    ));

    let file = tempfile::NamedTempFile::new().unwrap();
    let conf = LibbitcoinDConf {
        tmpdir: Some(file.path().to_path_buf()),
        max_retries: 1,
        ..LibbitcoinDConf::default()
    };
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(&program, &conf),
        Err(Error::Io(_))
    ));
}

#[test]
fn libbitcoin_retries_when_process_exits_before_rpc_is_ready() {
    let (_program_directory, program) = test_program("sleep 1", true);
    let data_directory = tempfile::tempdir().unwrap();
    let conf = LibbitcoinDConf {
        staticdir: Some(data_directory.path().to_path_buf()),
        max_retries: 1,
        ..LibbitcoinDConf::default()
    };
    assert!(matches!(
        LibbitcoinD::from_bin_with_conf(&program, &conf),
        Err(Error::StartupAttemptsExhausted(1))
    ));
    assert!(data_directory.path().join("bs.cfg").is_file());
    assert!(data_directory.path().join(".cookie").is_file());
}

#[test]
fn libbitcoin_rpc_methods_and_trait_dispatch() {
    let genesis = genesis_block(Network::Bitcoin).block_hash();
    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![json!(7), json!(genesis.to_string()), json!(3), json!(true)],
        vec![],
    );
    assert_eq!(<LibbitcoinD as Node>::get_chain_tip(&server).unwrap(), 7);
    assert_eq!(
        <LibbitcoinD as Node>::get_block_hash(&server, 0).unwrap(),
        genesis
    );
    assert_eq!(<LibbitcoinD as Node>::get_peer_count(&server).unwrap(), 3);
    assert_eq!(
        <LibbitcoinD as Node>::call(&server, "example", &[]).unwrap(),
        json!(true)
    );
    assert_eq!(
        <LibbitcoinD as Node>::get_rpc_socket(&server),
        server.get_rpc_socket()
    );
    assert_eq!(
        <LibbitcoinD as Node>::get_working_directory(&server),
        server.get_working_directory()
    );
    assert_eq!(
        <LibbitcoinD as Node>::get_p2p_socket(&server),
        server.get_p2p_socket()
    );
    assert_eq!(
        <LibbitcoinD as Indexer>::get_working_directory(&server),
        server.get_working_directory()
    );
    assert_eq!(
        <LibbitcoinD as Indexer>::get_config(&server),
        server.get_config()
    );
    assert!(std::ptr::eq(
        <LibbitcoinD as Indexer>::get_electrum_client(&server),
        server.get_electrum_client()
    ));
    assert_eq!(
        <LibbitcoinD as Indexer>::get_electrum_socket(&server),
        server.get_electrum_socket()
    );
    assert_eq!(
        <LibbitcoinD as Indexer>::get_electrum_url(&server),
        server.get_electrum_url()
    );
    assert_eq!(
        server.get_electrum_url(),
        format!("tcp://{}", server.get_electrum_socket())
    );
    assert_eq!(server.get_esplora_url(), server.get_esplora_client().url());
    assert_eq!(server.get_esplora_socket().port(), 1);
    assert_eq!(<LibbitcoinD as Indexer>::get_pid(&server), server.get_pid());
    assert_eq!(
        <LibbitcoinD as Node>::get_config(&server),
        server.get_config()
    );
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_exposes_a_working_esplora_client() {
    let (mut server, rpc_server, electrum_server) = scripted_instance(vec![], vec![]);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    let http_server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let count = stream.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /blocks/tip/height "));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n7")
            .unwrap();
    });
    server.esplora_socket = socket;
    server.esplora_client =
        esplora_client::Builder::new(&server.get_esplora_url()).build_blocking();
    assert_eq!(server.get_esplora_client().get_height().unwrap(), 7);
    http_server.join().unwrap();
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_rejects_malformed_rpc_results() {
    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![
            json!("seven"),
            json!(-1),
            json!("invalid"),
            json!(42),
            json!("three"),
        ],
        vec![],
    );
    assert!(matches!(
        server.get_chain_tip(),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        server.get_chain_tip(),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        server.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        server.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        server.get_peer_count(),
        Err(Error::UnexpectedResponse(_))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_rejects_out_of_range_rpc_numbers_and_rpc_errors() {
    let overflow = u64::from(u32::MAX) + 1;
    let (server, rpc_server, electrum_server) =
        scripted_instance(vec![json!(overflow), json!(overflow)], vec![]);
    assert!(matches!(
        server.get_chain_tip(),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        server.get_peer_count(),
        Err(Error::UnexpectedResponse(_))
    ));
    rpc_server.join().unwrap();
    assert!(matches!(
        server.get_chain_tip(),
        Err(Error::Node(NodeError::JsonRpc(_)))
    ));
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_reports_config_write_failure() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        LibbitcoinD::write_config(
            root.path(),
            root.path(),
            SocketAddr::from(([127, 0, 0, 1], 1)),
            SocketAddr::from(([127, 0, 0, 1], 2)),
            SocketAddr::from(([127, 0, 0, 1], 3)),
            SocketAddr::from(([127, 0, 0, 1], 4)),
        ),
        Err(Error::Io(_))
    ));
}

#[test]
fn libbitcoin_reports_unsupported_node_and_indexer_commands() {
    let (server, rpc_server, electrum_server) = scripted_instance(vec![], vec![]);
    for result in [
        <LibbitcoinD as Node>::generate(&server, 1).map(|_| ()),
        <LibbitcoinD as Node>::get_filter_tip(&server).map(|_| ()),
        <LibbitcoinD as Node>::has_peer(&server, server.get_p2p_socket()).map(|_| ()),
        <LibbitcoinD as Node>::add_peer(&server, server.get_p2p_socket()),
    ] {
        assert!(matches!(
            result,
            Err(Error::Node(NodeError::UnsupportedCommand { .. }))
        ));
    }
    assert!(matches!(
        <LibbitcoinD as Indexer>::trigger(&server),
        Err(Error::Indexer(IndexerError::UnsupportedCommand { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_waits_for_electrum_tip_and_mempool_history() {
    let genesis = genesis_block(Network::Bitcoin);
    let header_hex = serialize(&genesis.header).to_lower_hex_string();
    let notification = json!({ "height": 0, "hex": header_hex });
    let txid = Txid::all_zeros();
    let history = json!([{ "tx_hash": txid.to_string(), "height": 0 }]);
    let (mut server, rpc_server, electrum_server) = scripted_instance(
        vec![json!(0), json!(genesis.block_hash().to_string())],
        vec![
            Some(Ok(notification)),
            Some(Ok(json!(header_hex))),
            Some(Ok(history)),
        ],
    );
    <LibbitcoinD as Indexer>::wait_until_caught_up(&server, &server, Some(Duration::from_secs(1)))
        .unwrap();
    <LibbitcoinD as Indexer>::wait_until_mempool_tx(
        &server,
        &ScriptBuf::new(),
        txid,
        Some(Duration::from_secs(1)),
    )
    .unwrap();
    let directory = server.get_working_directory();
    assert!(directory.exists());
    assert!(
        <LibbitcoinD as Indexer>::stop(&mut server)
            .unwrap()
            .success()
    );
    drop(server);
    assert!(!directory.exists());
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_waits_report_timeout_and_electrum_errors() {
    let genesis: BlockHash = genesis_block(Network::Bitcoin).block_hash();
    let (server, rpc_server, electrum_server) = scripted_instance(vec![], vec![]);
    assert!(matches!(
        server.wait_until_tip(0, genesis, Some(Duration::ZERO)),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    assert!(matches!(
        server.wait_until_mempool_tx(&ScriptBuf::new(), Txid::all_zeros(), Some(Duration::ZERO)),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();

    let protocol_error = json!({ "code": 1, "message": "unavailable" });
    let (server, rpc_server, electrum_server) =
        scripted_instance(vec![], vec![Some(Err(protocol_error))]);
    assert!(matches!(
        server.wait_until_tip(0, genesis, Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();

    let protocol_error = json!({ "code": 1, "message": "unavailable" });
    let (server, rpc_server, electrum_server) =
        scripted_instance(vec![], vec![Some(Err(protocol_error))]);
    assert!(matches!(
        server.wait_until_mempool_tx(
            &ScriptBuf::new(),
            Txid::all_zeros(),
            Some(Duration::from_secs(1))
        ),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_waits_require_matching_tip_and_unconfirmed_transaction() {
    let genesis = genesis_block(Network::Bitcoin);
    let header_hex = serialize(&genesis.header).to_lower_hex_string();
    let wrong_hash = BlockHash::all_zeros();
    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![],
        vec![
            Some(Ok(json!({ "height": 0, "hex": header_hex }))),
            Some(Ok(json!(header_hex))),
        ],
    );
    assert!(matches!(
        <LibbitcoinD as Indexer>::wait_until_tip(
            &server,
            0,
            wrong_hash,
            Some(Duration::from_millis(1))
        ),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();

    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![],
        vec![Some(Ok(json!({ "height": 0, "hex": header_hex })))],
    );
    assert!(matches!(
        server.wait_until_tip(1, genesis.block_hash(), Some(Duration::from_millis(1))),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();

    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![],
        vec![
            Some(Ok(json!({ "height": 0, "hex": header_hex }))),
            Some(Err(json!({ "code": 1, "message": "header unavailable" }))),
        ],
    );
    assert!(matches!(
        server.wait_until_tip(0, genesis.block_hash(), Some(Duration::from_secs(1))),
        Err(Error::Indexer(IndexerError::UnresponsiveIndexer { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();

    let txid = Txid::all_zeros();
    let (server, rpc_server, electrum_server) = scripted_instance(
        vec![],
        vec![Some(Ok(
            json!([{ "tx_hash": txid.to_string(), "height": 1 }]),
        ))],
    );
    assert!(matches!(
        server.wait_until_mempool_tx(&ScriptBuf::new(), txid, Some(Duration::from_millis(1))),
        Err(Error::Indexer(IndexerError::IndexingTimeout { .. }))
    ));
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}

#[test]
fn libbitcoin_stop_handles_closed_stdin() {
    let (mut server, rpc_server, electrum_server) = scripted_instance(vec![], vec![]);
    drop(server.process.stdin.take());
    // The fixture's `read line` exits unsuccessfully at EOF; stop still returns its status.
    assert!(!server.stop().unwrap().success());
    rpc_server.join().unwrap();
    electrum_server.join().unwrap();
}
