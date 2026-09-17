// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration, failure, and lifecycle tests for [`FrigateD`].

use core::net::SocketAddr;
use core::time::Duration;
use std::path::PathBuf;
use std::process::ExitStatus;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Script;
use corepc_client::bitcoin::ScriptBuf;
use corepc_client::bitcoin::Txid;
use corepc_client::bitcoin::consensus::serialize;
use corepc_client::bitcoin::constants::genesis_block;
use corepc_client::bitcoin::hashes::Hash;
use corepc_client::bitcoin::hex::DisplayHex;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use serde_json::Value;

use super::FrigateD;
use super::FrigateDConf;
use super::get_frigate_path;
use super::is_incomplete_read;
use super::wait_for_client;
use crate::Error;
use crate::SPAWN_ATTEMPTS;
use crate::indexer::Indexer;
use crate::indexer::IndexerError;
#[cfg(all(feature = "bitcoind", feature = "romanz_electrs"))]
use crate::indexer::romanz_electrsd::RomanzElectrsD;
use crate::indexer::test::FakeNode;
use crate::indexer::test::scripted_electrum_socket;
#[cfg(unix)]
use crate::indexer::test::test_program;
use crate::node::Node;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "btcd")]
use crate::node::btcd::BtcD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Minimal indexer for constructor validation tests.
#[derive(Debug)]
struct StubIndexer<const IS_FRIGATE: bool>;

/// Wrap a scripted Electrum client with the Frigate state needed by wait methods.
fn scripted_frigate<'a>(
    backend: &'a StubIndexer<false>,
    responses: Vec<Option<Result<Value, Value>>>,
) -> (
    FrigateD<'a, StubIndexer<false>>,
    std::thread::JoinHandle<()>,
) {
    let (socket, server) = scripted_electrum_socket(responses);
    let client = RawClient::new(socket, Some(Duration::from_secs(1)), None).unwrap();
    let process = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    let working_directory = crate::init_data_dir(None, None, "halfin-frigate-test-").unwrap();
    (
        FrigateD {
            process,
            backend_indexer: backend,
            client,
            working_directory,
            config: FrigateDConf::default(),
            electrum_socket: socket,
        },
        server,
    )
}

#[rustfmt::skip]
impl<const IS_FRIGATE: bool> Indexer for StubIndexer<IS_FRIGATE> {
    type Config = ();
    fn get_name() -> &'static str { if IS_FRIGATE { "FrigateD" } else { "StubIndexer" } }
    fn get_bin_name() -> &'static str { "stub-indexer" }
    fn trigger(&self) -> Result<(), Error> { Ok(()) }
    fn stop(&mut self) -> Result<ExitStatus, Error> { unreachable!() }
    fn get_pid(&self) -> u32 { 0 }
    fn get_working_directory(&self) -> PathBuf { PathBuf::new() }
    fn get_config(&self) -> &() { &() }
    fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> { unreachable!() }
    fn get_electrum_socket(&self) -> SocketAddr { SocketAddr::from(([127, 0, 0, 1], 50001)) }
    fn get_electrum_url(&self) -> String { self.get_electrum_socket().to_string() }
    fn wait_until_caught_up(&self, _node: &impl Node, _timeout: Option<Duration>) -> Result<(), Error> { unreachable!() }
    fn wait_until_tip(&self, _height: u32, _hash: BlockHash, _timeout: Option<Duration>) -> Result<(), Error> { unreachable!() }
    fn wait_until_mempool_tx(&self, _spk: &Script, _txid: Txid, _timeout: Option<Duration>) -> Result<(), Error> { unreachable!() }
}

use corepc_client::bitcoin::Network;

use super::toml_string;
use super::validate_node_args;
use crate::node::NodeArgs;
use crate::node::PruneMode;

/// Check that Bitcoin's display names match Frigate's CLI network names.
#[test]
fn network_mapping() {
    assert_eq!(Network::Bitcoin.to_string(), "bitcoin");
    assert_eq!(Network::Testnet.to_string(), "testnet");
    assert_eq!(Network::Testnet4.to_string(), "testnet4");
    assert_eq!(Network::Signet.to_string(), "signet");
    assert_eq!(Network::Regtest.to_string(), "regtest");
}

/// Reject pruning and a disabled transaction index.
#[test]
fn core_index_requirements() {
    let mut args = NodeArgs {
        network: Network::Regtest,
        fixed_peers: Vec::new(),
        v2_transport: false,
        cbf_index: false,
        prune: PruneMode::Disabled,
        txindex: true,
    };
    assert!(validate_node_args(&args).is_ok());
    args.txindex = false;
    assert!(validate_node_args(&args).is_err());
    args.txindex = true;
    args.prune = PruneMode::Manual;
    assert!(validate_node_args(&args).is_err());
}

/// Quote arbitrary RPC credentials before writing TOML.
#[test]
fn credentials_are_escaped() {
    assert_eq!(toml_string("user:pass\"word"), "\"user:pass\\\"word\"");
}

/// Verify release metadata, local launcher selection, and standard configuration.
#[test]
fn frigate_binary_and_defaults() {
    let path = get_frigate_path().unwrap();
    assert!(path.is_absolute());
    assert!(path.is_file());
    assert_eq!(FrigateD::<StubIndexer<false>>::get_name(), "FrigateD");
    assert_eq!(FrigateD::<StubIndexer<false>>::get_bin_name(), "frigate");
    assert_eq!(FrigateDConf::default().max_retries, SPAWN_ATTEMPTS);
}

/// Reject a Frigate indexer as the Electrum backend before file or node access.
#[test]
fn frigate_rejects_another_frigate() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    assert!(matches!(
        FrigateD::from_bin("missing", &node, &StubIndexer::<true>),
        Err(Error::Indexer(IndexerError::InvalidConfiguration(_)))
    ));
}

/// Verify launcher path validation and zero startup attempts.
#[test]
fn frigate_validates_binary_path_and_start_attempts() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    let backend = StubIndexer::<false>;
    assert!(matches!(
        FrigateD::from_bin("frigate", &node, &backend),
        Err(Error::BinaryPathNotAbsolute { .. })
    ));
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        FrigateD::from_bin(directory.path().join("missing"), &node, &backend),
        Err(Error::BinaryPathNotFile { .. })
    ));
    node.write_cookie("user:password");
    let config = FrigateDConf {
        max_retries: 0,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(get_frigate_path().unwrap(), &node, &backend, &config),
        Err(Error::StartupAttemptsExhausted(0))
    ));
}

/// Reject Frigate-owned CLI options, including attached short forms.
#[test]
fn frigate_rejects_owned_raw_arguments() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    let backend = StubIndexer::<false>;
    for raw_arg in ["-d", "--dir=/tmp/frigate", "-nregtest", "--network=signet"] {
        let config = FrigateDConf {
            raw_args: vec![raw_arg.to_string()],
            ..FrigateDConf::default()
        };
        assert!(matches!(
            FrigateD::from_bin_with_conf("missing", &node, &backend, &config),
            Err(Error::Indexer(IndexerError::ConflictingArgument(_)))
        ));
    }
}

/// Reject pruning and a missing transaction index during construction.
#[test]
fn frigate_rejects_incompatible_node_settings() {
    let backend = StubIndexer::<false>;
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }))
        .with_prune(PruneMode::Automatic(550));
    assert!(matches!(
        FrigateD::from_bin(get_frigate_path().unwrap(), &node, &backend),
        Err(Error::Indexer(IndexerError::InvalidConfiguration(_)))
    ));
    let node =
        FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 })).with_txindex(false);
    assert!(matches!(
        FrigateD::from_bin(get_frigate_path().unwrap(), &node, &backend),
        Err(Error::Indexer(IndexerError::InvalidConfiguration(_)))
    ));
}

/// Reject btcd before creating a Frigate data directory.
#[cfg(feature = "btcd")]
#[test]
fn frigate_rejects_btcd() {
    let node = BtcD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("frigate");
    let config = FrigateDConf {
        staticdir: Some(directory.clone()),
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(
            get_frigate_path().unwrap(),
            &node,
            &StubIndexer::<false>,
            &config
        ),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "BtcD"
        }))
    ));
    assert!(!directory.exists());
}

/// Reject Floresta before creating a Frigate data directory.
#[cfg(feature = "florestad")]
#[test]
fn frigate_rejects_florestad() {
    let node = FlorestaD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("frigate");
    let config = FrigateDConf {
        staticdir: Some(directory.clone()),
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(
            get_frigate_path().unwrap(),
            &node,
            &StubIndexer::<false>,
            &config
        ),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "FlorestaD"
        }))
    ));
    assert!(!directory.exists());
}

/// Reject utreexod before creating a Frigate data directory.
#[cfg(feature = "utreexod")]
#[test]
fn frigate_rejects_utreexod() {
    let node = UtreexoD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("frigate");
    let config = FrigateDConf {
        staticdir: Some(directory.clone()),
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(
            get_frigate_path().unwrap(),
            &node,
            &StubIndexer::<false>,
            &config
        ),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!directory.exists());
}

/// Reject conflicting data directories before starting a process.
#[test]
fn frigate_rejects_two_data_directories() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    node.write_cookie("user:password");
    let backend = StubIndexer::<false>;
    let root = tempfile::tempdir().unwrap();
    let config = FrigateDConf {
        tmpdir: Some(root.path().to_path_buf()),
        staticdir: Some(root.path().join("persistent")),
        max_retries: 1,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(get_frigate_path().unwrap(), &node, &backend, &config),
        Err(Error::BothDirsSpecified)
    ));
}

/// Classify expected transport truncation without hiding a server error.
#[test]
fn frigate_classifies_incomplete_reads() {
    use std::io::Error as IoError;
    use std::io::ErrorKind;
    for kind in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::UnexpectedEof,
        ErrorKind::BrokenPipe,
    ] {
        assert!(is_incomplete_read(&electrum_client::Error::IOError(
            IoError::from(kind)
        )));
    }
    assert!(!is_incomplete_read(&electrum_client::Error::Message(
        "error".to_string()
    )));
}

/// Report a missing block tip after polling a valid but different header.
#[test]
fn frigate_tip_wait_times_out_on_wrong_hash() {
    let backend = StubIndexer::<false>;
    let header = genesis_block(Network::Regtest).header;
    let header_hex = serialize(&header).to_lower_hex_string();
    let (frigate, server) =
        scripted_frigate(&backend, vec![Some(Ok(serde_json::json!(header_hex)))]);
    let error = frigate
        .wait_until_tip(0, BlockHash::all_zeros(), Some(Duration::from_millis(50)))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));
    drop(frigate);
    server.join().unwrap();
}

/// Report a missing mempool transaction after an empty history response.
#[test]
fn frigate_mempool_wait_times_out_on_empty_history() {
    let backend = StubIndexer::<false>;
    let (frigate, server) = scripted_frigate(&backend, vec![Some(Ok(serde_json::json!([])))]);
    let error = frigate
        .wait_until_mempool_tx(
            &ScriptBuf::new(),
            Txid::all_zeros(),
            Some(Duration::from_millis(50)),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::IndexingTimeout { .. })
    ));
    drop(frigate);
    server.join().unwrap();
}

/// Preserve Electrum server errors with Frigate context for both wait methods.
#[test]
fn frigate_waits_report_electrum_errors() {
    let backend = StubIndexer::<false>;
    let response = Some(Err(
        serde_json::json!({"code": -1, "message": "unavailable"}),
    ));
    let (frigate, server) = scripted_frigate(&backend, vec![response.clone()]);
    let error = frigate
        .wait_until_tip(0, BlockHash::all_zeros(), Some(Duration::from_secs(1)))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::UnresponsiveIndexer { .. })
    ));
    drop(frigate);
    server.join().unwrap();

    let (frigate, server) = scripted_frigate(&backend, vec![response]);
    let error = frigate
        .wait_until_mempool_tx(
            &ScriptBuf::new(),
            Txid::all_zeros(),
            Some(Duration::from_secs(1)),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Indexer(IndexerError::UnresponsiveIndexer { .. })
    ));
    drop(frigate);
    server.join().unwrap();
}

/// Exercise directory, executable, retry, and client startup failures with tiny programs.
#[cfg(unix)]
#[test]
fn frigate_reports_startup_failures() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));
    node.write_cookie("user:password");
    let backend = StubIndexer::<false>;
    let (_program_dir, program) = test_program("exit 1", true);
    let config = FrigateDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(&program, &node, &backend, &config),
        Err(Error::Io(_))
    ));

    let (_program_dir, program) = test_program("exit 1", false);
    let config = FrigateDConf {
        max_retries: 1,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(&program, &node, &backend, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_dir, program) = test_program("exit 1", true);
    let config = FrigateDConf {
        max_retries: 2,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(&program, &node, &backend, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    // Stay alive past the initial check, then exit while the client is waiting.
    let (_program_dir, program) = test_program("sleep 1; exit 1", true);
    let config = FrigateDConf {
        max_retries: 1,
        ..FrigateDConf::default()
    };
    assert!(matches!(
        FrigateD::from_bin_with_conf(&program, &node, &backend, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));

    let socket = SocketAddr::from(([127, 0, 0, 1], 0));
    let mut process = std::process::Command::new("true").spawn().unwrap();
    process.wait().unwrap();
    assert!(matches!(
        wait_for_client(socket, &mut process, Duration::from_millis(50)),
        Err(Error::ClientSetupTimeout)
    ));

    let mut process = std::process::Command::new("sleep")
        .arg("2")
        .spawn()
        .unwrap();
    assert!(matches!(
        wait_for_client(socket, &mut process, Duration::from_millis(50)),
        Err(Error::ClientSetupTimeout)
    ));
    process.kill().unwrap();
    process.wait().unwrap();
}

/// Verify Frigate starts over a real backend and cleans up temporary data on drop.
#[cfg(all(feature = "bitcoind", feature = "romanz_electrs"))]
#[test]
fn frigate_lifecycle_cleans_temporary_directory() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(3).unwrap();
    let backend = RomanzElectrsD::new(&bitcoind).unwrap();
    backend.wait_until_caught_up(&bitcoind, None).unwrap();
    let mut frigate = FrigateD::new(&bitcoind, &backend).unwrap();
    let directory = frigate.get_working_directory();
    assert!(directory.join("regtest/config.toml").is_file());
    assert!(frigate.get_pid() > 0);
    assert!(frigate.get_electrum_socket().ip().is_loopback());
    frigate.get_electrum_client().ping().unwrap();
    frigate.wait_until_caught_up(&bitcoind, None).unwrap();
    frigate.stop().unwrap();
    drop(frigate);
    assert!(!directory.exists());
}

/// Verify a persistent Frigate database can be reopened with the same backend.
#[cfg(all(feature = "bitcoind", feature = "romanz_electrs"))]
#[test]
fn frigate_static_directory_survives_restart() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(3).unwrap();
    let backend = RomanzElectrsD::new(&bitcoind).unwrap();
    backend.wait_until_caught_up(&bitcoind, None).unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("frigate");
    let config = FrigateDConf {
        staticdir: Some(directory.clone()),
        ..FrigateDConf::default()
    };

    let mut frigate = FrigateD::new_with_conf(&bitcoind, &backend, &config).unwrap();
    frigate.wait_until_caught_up(&bitcoind, None).unwrap();
    frigate.stop().unwrap();
    drop(frigate);
    assert!(directory.join("regtest/config.toml").is_file());
    assert!(directory.join("regtest/db/frigate.duckdb").is_file());

    let mut frigate = FrigateD::new_with_conf(&bitcoind, &backend, &config).unwrap();
    frigate.wait_until_caught_up(&bitcoind, None).unwrap();
    assert_eq!(frigate.get_working_directory(), directory);
    frigate.stop().unwrap();
    drop(frigate);
    assert!(directory.is_dir());
}

/// Verify Frigate follows new blocks and the replacement tip after a reorganization.
#[cfg(all(feature = "bitcoind", feature = "romanz_electrs"))]
#[test]
fn frigate_follows_blocks_and_reorganizations() {
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(3).unwrap();
    let backend = RomanzElectrsD::new(&bitcoind).unwrap();
    backend.wait_until_caught_up(&bitcoind, None).unwrap();
    let frigate = FrigateD::new(&bitcoind, &backend).unwrap();
    frigate.wait_until_caught_up(&bitcoind, None).unwrap();

    bitcoind.generate(2).unwrap();
    frigate.wait_until_caught_up(&bitcoind, None).unwrap();
    let height = bitcoind.get_chain_tip().unwrap();
    let original_hash = bitcoind.get_block_hash(height).unwrap();

    bitcoind.invalidate_blocks(1).unwrap();
    bitcoind.generate(1).unwrap();
    let replacement_hash = bitcoind.get_block_hash(height).unwrap();
    assert_ne!(original_hash, replacement_hash);
    frigate
        .wait_until_tip(height, replacement_hash, None)
        .unwrap();
}
