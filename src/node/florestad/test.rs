// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`FlorestaD`].

use core::time::Duration;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::net::TcpListener;
#[cfg(unix)]
use std::process::Command;
use std::thread::JoinHandle;

use corepc_client::bitcoin::Network;
use electrum_client::ElectrumApi;

use super::Client;
use super::FlorestaD;
use super::FlorestaDConf;
use super::get_florestad_path;
use crate::Error;
#[cfg(feature = "utreexod")]
use crate::PERSISTENCE_BLOCK_COUNT;
use crate::WALLET_PUBKEY;
use crate::node::Node;
use crate::node::NodeError;
use crate::node::PruneMode;
#[cfg(feature = "utreexod")]
use crate::node::connect_and_sync;
use crate::node::test::scripted_json_rpc_server;
#[cfg(unix)]
use crate::node::test::test_program;
#[cfg(feature = "utreexod")]
use crate::node::test::wait_for_fixed_peers;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;
#[cfg(feature = "utreexod")]
use crate::node::wait_for_height_with_timeout;

#[cfg(feature = "utreexod")]
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Start an Electrum server that completes negotiation and drops the ping request.
fn electrum_server_without_ping_response() -> (core::net::SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let version_request: serde_json::Value = serde_json::from_str(&request).unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "id": version_request["id"].clone(),
                "result": ["halfin-test", "1.4"]
            })
        )
        .unwrap();

        request.clear();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
    });
    (socket, handle)
}

/// Verify binary-path validation and zero start attempts.
#[test]
fn florestad_validates_binary_path_and_start_attempts() {
    let error = FlorestaD::from_bin("florestad").unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotAbsolute { .. }));

    let root = tempfile::tempdir().unwrap();
    let error = FlorestaD::from_bin(root.path().join("missing-florestad")).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotFile { .. }));

    let config = FlorestaDConf {
        max_retries: 0,
        ..FlorestaDConf::default()
    };
    let error = FlorestaD::from_bin_with_conf(get_florestad_path().unwrap(), &config).unwrap_err();
    assert!(matches!(error, Error::StartupAttemptsExhausted(0)));
}

/// Verify directory, spawn, retry, and client-timeout startup failures.
#[cfg(unix)]
#[test]
fn florestad_reports_test_program_startup_failures() {
    let (_program_directory, program) = test_program("exit 1", true);
    let config = FlorestaDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..FlorestaDConf::default()
    };
    assert!(matches!(
        FlorestaD::from_bin_with_conf(&program, &config),
        Err(Error::Io(_))
    ));

    let (_program_directory, program) = test_program("exit 1", false);
    let config = FlorestaDConf {
        max_retries: 1,
        ..FlorestaDConf::default()
    };
    assert!(matches!(
        FlorestaD::from_bin_with_conf(&program, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_directory, program) = test_program("exit 1", true);
    let config = FlorestaDConf {
        max_retries: 2,
        ..FlorestaDConf::default()
    };
    assert!(matches!(
        FlorestaD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    let (_program_directory, program) = test_program("exec sleep 30", true);
    let config = FlorestaDConf {
        max_retries: 1,
        ..FlorestaDConf::default()
    };
    assert!(matches!(
        FlorestaD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));
}

/// Verify malformed successful RPC results are rejected by typed helpers.
#[test]
fn florestad_rejects_malformed_rpc_results() {
    let mut florestad = FlorestaD::new().unwrap();
    let (socket, server) = scripted_json_rpc_server(vec![
        serde_json::json!("not-a-height"),
        serde_json::json!(0),
        serde_json::json!("not-a-block-hash"),
        serde_json::Value::Null,
        serde_json::Value::Null,
    ]);
    florestad.client = Client::new(&format!("http://{socket}"));

    assert!(matches!(
        florestad.get_chain_tip(),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        florestad.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        florestad.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        florestad.has_peer("127.0.0.1:18444".parse().unwrap()),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        florestad.get_peer_count(),
        Err(Error::UnexpectedResponse(_))
    ));
    server.join().unwrap();
}

/// Verify Floresta startup, process data, and RPC data.
#[test]
fn florestad_starts() {
    let config = FlorestaDConf {
        raw_args: vec!["--debug".to_string()],
        ..FlorestaDConf::default()
    };
    let mut florestad =
        FlorestaD::from_bin_with_conf(get_florestad_path().unwrap(), &config).unwrap();

    assert!(florestad.get_pid() > 0);
    assert!(florestad.get_working_directory().is_dir());
    assert_eq!(florestad.get_config(), &config);
    assert_eq!(Node::get_config(&florestad), &config);
    assert_eq!(config.as_ref(), &config.args);
    let _ = florestad.get_rpc_client();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_socket = listener.local_addr().unwrap();
    drop(listener);
    assert!(matches!(
        FlorestaD::wait_for_rpc_client(
            &format!("http://{unavailable_socket}"),
            Duration::from_millis(250),
        ),
        Err(Error::ClientSetupTimeout)
    ));
    assert!(matches!(
        FlorestaD::wait_for_electrum_client(
            unavailable_socket,
            &mut florestad.process,
            Duration::from_millis(250),
        ),
        Err(Error::Node(NodeError::UnresponsiveNode { .. }))
    ));
    let (socket, server) = electrum_server_without_ping_response();
    assert!(matches!(
        FlorestaD::wait_for_electrum_client(
            socket,
            &mut florestad.process,
            Duration::from_millis(250),
        ),
        Err(Error::Node(NodeError::UnresponsiveNode { .. }))
    ));
    server.join().unwrap();
    #[cfg(unix)]
    {
        let mut exited_process = Command::new("true").spawn().unwrap();
        exited_process.wait().unwrap();
        assert!(matches!(
            FlorestaD::wait_for_electrum_client(
                unavailable_socket,
                &mut exited_process,
                Duration::from_secs(1),
            ),
            Err(Error::ClientSetupTimeout)
        ));
    }
    assert_eq!(florestad.get_chain_tip().unwrap(), 0);
    Node::get_block_hash(&florestad, 0).unwrap();
    assert_eq!(florestad.get_peer_count().unwrap(), 0);
    assert_eq!(
        florestad.get_electrum_url(),
        florestad.get_electrum_socket().to_string()
    );
    florestad.get_electrum_client().ping().unwrap();
    assert!(matches!(
        florestad.generate(1),
        Err(Error::Node(NodeError::UnsupportedCommand {
            node: "FlorestaD",
            command: "generate"
        }))
    ));
    assert!(matches!(
        florestad.get_filter_tip(),
        Err(Error::Node(NodeError::UnsupportedCommand {
            node: "FlorestaD",
            command: "get_filter_tip"
        }))
    ));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Node::get_p2p_socket(&florestad)
        }))
        .is_err()
    );
}

/// Verify that Floresta synchronizes with a chain that `utreexod` mines.
#[cfg(feature = "utreexod")]
#[test]
fn florestad_syncs_from_utreexod() {
    const HISTORICAL_BLOCKS: u32 = 6;
    const LIVE_BLOCKS: u32 = 4;

    let utreexod = UtreexoD::new().unwrap();
    let mut block_hashes = utreexod.generate(HISTORICAL_BLOCKS).unwrap();

    let florestad = FlorestaD::new().unwrap();
    let socket = utreexod.get_p2p_socket();

    assert_eq!(florestad.get_chain_tip().unwrap(), 0);
    connect_and_sync(&florestad, &utreexod).unwrap();
    assert!(florestad.has_peer(socket).unwrap());
    assert_eq!(florestad.get_peer_count().unwrap(), 1);

    let height = HISTORICAL_BLOCKS + LIVE_BLOCKS;
    for next_height in HISTORICAL_BLOCKS + 1..=height {
        block_hashes.extend(utreexod.generate(1).unwrap());
        wait_for_height_with_timeout(&utreexod, next_height, SYNC_TIMEOUT).unwrap();
        wait_for_height_with_timeout(&florestad, next_height, SYNC_TIMEOUT).unwrap();
    }

    assert_eq!(florestad.get_chain_tip().unwrap(), height);
    for (height, block_hash) in (1..=height).zip(&block_hashes) {
        assert_eq!(florestad.get_block_hash(height).unwrap(), *block_hash);
    }

    let blockchain_info = florestad.call("getblockchaininfo", &[]).unwrap();
    let block_hash = block_hashes.last().unwrap().to_string();
    assert_eq!(blockchain_info["chain"].as_str(), Some("regtest"));
    assert_eq!(blockchain_info["height"].as_u64(), Some(u64::from(height)));
    assert_eq!(
        blockchain_info["best_block"].as_str(),
        Some(block_hash.as_str())
    );

    let tip = florestad
        .get_electrum_client()
        .block_headers_subscribe()
        .unwrap();
    assert_eq!(tip.height as u32, height);
    assert_eq!(
        tip.header.block_hash(),
        florestad.get_block_hash(height).unwrap()
    );
    florestad.client.uptime().unwrap();
}

/// Verify that [`FlorestaD`] connects to its fixed peer during startup.
#[cfg(feature = "utreexod")]
#[test]
fn florestad_connects_to_fixed_peer() {
    let peer = UtreexoD::new().unwrap();
    let peers = [peer.get_p2p_socket()];

    let mut config = FlorestaDConf::default();
    config.args.fixed_peers = peers.to_vec();
    let florestad = FlorestaD::new_with_conf(&config).unwrap();

    wait_for_fixed_peers(&florestad, &peers, Duration::from_secs(15));
    assert_eq!(florestad.get_peer_count().unwrap(), peers.len() as u32);

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_socket = listener.local_addr().unwrap();
    drop(listener);
    assert!(matches!(
        florestad.add_peer(unavailable_socket),
        Err(Error::Node(NodeError::ConnectionTimeout(timeout)))
            if timeout == crate::CONNECTION_TIMEOUT
    ));
}

#[test]
fn florestad_default_configuration_is_isolated_regtest() {
    let config = FlorestaDConf::default();

    assert_eq!(config.args.network, Network::Regtest);
    assert!(config.args.fixed_peers.is_empty());
    assert!(config.args.v2_transport);
    assert!(config.args.cbf_index);
    assert_eq!(config.args.prune, PruneMode::Disabled);
    assert!(!config.args.txindex);
    assert!(!config.florestad_args.dns_seeds);
    assert!(!config.florestad_args.allow_v1_fallback);
    assert!(!config.florestad_args.assume_utreexo);
    assert!(!config.florestad_args.backfill);
    assert!(config.florestad_args.descriptors.is_empty());
    assert_eq!(
        FlorestaD::configured_args(&config).unwrap(),
        [
            "--network=regtest",
            "--disable-dns-seeds",
            "--no-assume-utreexo",
            "--no-backfill",
        ]
    );
}

#[test]
fn florestad_renders_fixed_peer() {
    let mut config = FlorestaDConf::default();
    config.args.fixed_peers = vec!["127.0.0.1:18444".parse().unwrap()];

    let args = FlorestaD::configured_args(&config).unwrap();

    assert!(args.contains(&"--connect=127.0.0.1:18444".to_string()));
}

#[test]
fn florestad_rejects_multiple_fixed_peers() {
    let mut config = FlorestaDConf::default();
    config.args.fixed_peers = ["127.0.0.1:18444", "127.0.0.1:18445"]
        .map(|peer| peer.parse().unwrap())
        .to_vec();

    assert!(matches!(
        FlorestaD::configured_args(&config),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));
}

#[test]
fn florestad_renders_supported_flags() {
    let mut config = FlorestaDConf::default();
    config.args.network = Network::Testnet4;
    config.args.v2_transport = false;
    config.args.cbf_index = false;
    config.florestad_args.dns_seeds = true;
    config.florestad_args.allow_v1_fallback = true;
    config.florestad_args.assume_utreexo = true;
    config.florestad_args.backfill = true;
    config.florestad_args.descriptors = [
        format!("wpkh({WALLET_PUBKEY})"),
        format!("sh(wpkh({WALLET_PUBKEY}))"),
    ]
    .map(|descriptor| descriptor.parse().unwrap())
    .to_vec();

    assert_eq!(
        FlorestaD::configured_args(&config).unwrap(),
        [
            "--network=testnet4".to_string(),
            "--no-cfilters".to_string(),
            "--allow-v1-fallback".to_string(),
            format!(
                "--wallet-descriptor={}",
                config.florestad_args.descriptors[0]
            ),
            format!(
                "--wallet-descriptor={}",
                config.florestad_args.descriptors[1]
            ),
        ]
    );
}

#[test]
fn florestad_renders_v1_fallback_independently_from_manual_peer_transport() {
    let mut config = FlorestaDConf::default();
    config.args.v2_transport = false;

    assert!(
        !FlorestaD::configured_args(&config)
            .unwrap()
            .contains(&"--allow-v1-fallback".to_string())
    );

    config.args.v2_transport = true;
    config.florestad_args.allow_v1_fallback = true;

    assert!(
        FlorestaD::configured_args(&config)
            .unwrap()
            .contains(&"--allow-v1-fallback".to_string())
    );
}

#[test]
fn florestad_rejects_unsupported_typed_configuration() {
    let mut config = FlorestaDConf::default();
    config.args.prune = PruneMode::Automatic(550);
    assert!(matches!(
        FlorestaD::configured_args(&config),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));

    config.args.prune = PruneMode::Disabled;
    config.args.txindex = true;
    assert!(matches!(
        FlorestaD::configured_args(&config),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));
}

#[test]
fn florestad_rejects_owned_raw_arguments() {
    for arg in [
        "--network=bitcoin",
        "-n=signet",
        "-nregtest",
        "--data-dir=/tmp/floresta",
        "--rpc-address=127.0.0.1:8332",
        "--electrum-address=127.0.0.1:50001",
        "--connect=127.0.0.1:18444",
        "--no-cfilters",
        "--disable-dns-seeds",
        "--no-assume-utreexo",
        "--no-backfill",
        "--wallet-descriptor=raw(51)",
        "--allow-v1-fallback",
        "--daemon",
    ] {
        let config = FlorestaDConf {
            raw_args: vec![arg.to_string()],
            ..FlorestaDConf::default()
        };
        assert!(matches!(
            FlorestaD::configured_args(&config),
            Err(Error::Node(NodeError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }
}

/// Verify process state, typed RPC access, shutdown, and temporary cleanup.
#[test]
fn florestad_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let config = FlorestaDConf {
        raw_args: vec!["--debug".to_string()],
        ..FlorestaDConf::default()
    };
    let mut florestad =
        FlorestaD::from_bin_with_conf(get_florestad_path().unwrap(), &config).unwrap();
    let directory = florestad.get_working_directory();

    assert!(florestad.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(florestad.get_config(), &config);
    assert!(florestad.get_rpc_socket().ip().is_loopback());
    assert!(florestad.get_electrum_socket().ip().is_loopback());
    assert_ne!(florestad.get_rpc_socket(), florestad.get_electrum_socket());
    florestad.client.uptime().unwrap();
    florestad.electrum_client.ping().unwrap();

    assert!(florestad.stop().unwrap().success());
    drop(florestad);
    assert!(!directory.exists());
}

/// Verify that a static directory retains chain state across a restart.
#[cfg(feature = "utreexod")]
#[test]
fn florestad_static_directory_restores_chain_state() {
    let utreexod = UtreexoD::new().unwrap();
    let block_hashes = utreexod.generate(PERSISTENCE_BLOCK_COUNT).unwrap();
    let block_hash = *block_hashes.last().unwrap();

    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("florestad");
    let config = FlorestaDConf {
        staticdir: Some(directory.clone()),
        ..FlorestaDConf::default()
    };

    let mut florestad = FlorestaD::new_with_conf(&config).unwrap();
    connect_and_sync(&florestad, &utreexod).unwrap();
    assert_eq!(florestad.get_chain_tip().unwrap(), PERSISTENCE_BLOCK_COUNT);
    assert!(florestad.stop().unwrap().success());
    drop(florestad);

    assert!(directory.is_dir());

    let mut florestad = FlorestaD::new_with_conf(&config).unwrap();
    assert_eq!(florestad.get_chain_tip().unwrap(), PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        florestad.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap(),
        block_hash
    );
    assert!(florestad.stop().unwrap().success());
    drop(florestad);
    assert!(directory.is_dir());
}
