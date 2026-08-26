// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`BtcD`].

use std::fs;
use std::str::FromStr;
use std::time::Duration;

use corepc_client::bitcoin::Address;
use corepc_client::bitcoin::Network;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v17::Client;

use super::BtcD;
use super::BtcDConf;
use super::DEFAULT_MINING_ADDRESS;
use super::get_btcd_path;
use crate::CONNECTION_TIMEOUT;
use crate::Error;
use crate::FILTER_BLOCK_COUNT;
use crate::PERSISTENCE_BLOCK_COUNT;
use crate::node::Node;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::node::connect;
use crate::node::test::scripted_json_rpc_server;
#[cfg(unix)]
use crate::node::test::test_program;
use crate::node::test::wait_for_fixed_peers;
use crate::node::wait_for_filter_height;
use crate::node::wait_for_height;

/// Verify binary-path validation and zero start attempts.
#[test]
fn btcd_validates_binary_path_and_start_attempts() {
    let error = BtcD::from_bin("btcd").unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotAbsolute { .. }));

    let root = tempfile::tempdir().unwrap();
    let error = BtcD::from_bin(root.path().join("missing-btcd")).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotFile { .. }));

    let config = BtcDConf {
        max_retries: 0,
        ..BtcDConf::default()
    };
    let error = BtcD::from_bin_with_conf(get_btcd_path().unwrap(), &config).unwrap_err();
    assert!(matches!(error, Error::StartupAttemptsExhausted(0)));
}

/// Verify directory, spawn, retry, and client-timeout startup failures.
#[cfg(unix)]
#[test]
fn btcd_reports_test_program_startup_failures() {
    let (_program_directory, program) = test_program("exit 1", true);
    let config = BtcDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..BtcDConf::default()
    };
    assert!(matches!(
        BtcD::from_bin_with_conf(&program, &config),
        Err(Error::Io(_))
    ));

    let (_program_directory, program) = test_program("exit 1", false);
    let config = BtcDConf {
        max_retries: 1,
        ..BtcDConf::default()
    };
    assert!(matches!(
        BtcD::from_bin_with_conf(&program, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_directory, program) = test_program("exit 1", true);
    let config = BtcDConf {
        max_retries: 2,
        ..BtcDConf::default()
    };
    assert!(matches!(
        BtcD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    let (_program_directory, program) = test_program("exec sleep 30", true);
    let config = BtcDConf {
        max_retries: 1,
        ..BtcDConf::default()
    };
    assert!(matches!(
        BtcD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));
}

/// Verify malformed successful RPC results are rejected by typed helpers.
#[test]
fn btcd_rejects_malformed_rpc_results() {
    let mut btcd = BtcD::new().unwrap();
    let (socket, server) = scripted_json_rpc_server(vec![
        serde_json::json!(0),
        serde_json::json!("not-a-block-hash"),
        serde_json::Value::Null,
        serde_json::json!([0]),
        serde_json::json!(["not-a-block-hash"]),
        serde_json::Value::Null,
    ]);
    btcd.client = Client::new_with_auth(
        &format!("http://{socket}"),
        Auth::UserPass("user".to_string(), "password".to_string()),
    )
    .unwrap();

    assert!(matches!(
        btcd.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        btcd.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        btcd.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        btcd.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        btcd.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        btcd.get_peer_count(),
        Err(Error::UnexpectedResponse(_))
    ));
    server.join().unwrap();
}

/// Verify [`BtcD`] startup, process data, and P2P data.
#[test]
fn btcd_starts() {
    let path = get_btcd_path().unwrap();
    let config = BtcDConf {
        raw_args: vec!["--debuglevel=info".to_string()],
        ..BtcDConf::default()
    };
    let btcd = BtcD::from_bin_with_conf(path, &config).unwrap();

    println!("PID: {}", btcd.get_pid());
    println!("Working Directory: {:?}", btcd.get_working_directory());
    println!("P2P Socket: {}", btcd.get_p2p_socket());
    assert_eq!(btcd.get_config(), &config);
    assert_eq!(Node::get_config(&btcd), &config);
    assert_eq!(config.as_ref(), &config.args);
    assert_eq!(BtcD::poll_interval(), crate::POLL_INTERVAL);
    let _ = btcd.get_rpc_client();
    assert!(matches!(
        BtcD::wait_for_client(
            &format!("http://{}", btcd.get_rpc_socket()),
            &Auth::None,
            Duration::from_millis(250),
        ),
        Err(Error::ClientSetupTimeout)
    ));
}

/// Verify startup without the optional transaction index.
#[test]
fn btcd_starts_without_txindex() {
    let mut config = BtcDConf::default();
    config.args.txindex = false;

    let btcd = BtcD::new_with_conf(&config).unwrap();
    assert!(btcd.get_pid() > 0);
}

/// Verify that `generate` mines the specified number of blocks.
#[test]
fn btcd_generate() {
    let btcd = BtcD::new().unwrap();

    assert_eq!(btcd.get_chain_tip().unwrap(), 0);
    assert_eq!(Node::get_chain_tip(&btcd).unwrap(), 0);
    Node::generate(&btcd, 10).unwrap();
    assert_eq!(btcd.get_chain_tip().unwrap(), 10);
}

#[test]
fn btcd_get_filter_height() {
    let btcd = BtcD::new().unwrap();

    btcd.generate(FILTER_BLOCK_COUNT).unwrap();
    wait_for_filter_height(&btcd, FILTER_BLOCK_COUNT).unwrap();

    assert_eq!(btcd.get_filter_tip().unwrap(), FILTER_BLOCK_COUNT);
}

/// Verify that [`BtcD::get_block_hash`] returns the correct hash at a specified height.
#[test]
fn btcd_get_block_hash() {
    let btcd = BtcD::new().unwrap();

    let block_hashes = btcd.generate(10).unwrap();

    assert_eq!(
        Node::get_block_hash(&btcd, 10).unwrap(),
        *block_hashes.last().unwrap()
    );
}

/// Verify a connection between two [`Node`](crate::node::Node) implementations through
/// [`connect`].
#[test]
fn btcd_addnode() {
    let btcd_alpha = BtcD::new().unwrap();
    let btcd_beta = BtcD::new().unwrap();

    assert_eq!(btcd_alpha.get_peer_count().unwrap(), 0);
    assert_eq!(btcd_beta.get_peer_count().unwrap(), 0);

    connect(&btcd_alpha, &btcd_beta).unwrap();

    assert_eq!(btcd_alpha.get_peer_count().unwrap(), 1);
    assert_eq!(btcd_beta.get_peer_count().unwrap(), 1);

    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_socket = listener.local_addr().unwrap();
    drop(listener);
    assert!(matches!(
        btcd_alpha.add_peer(unavailable_socket),
        Err(Error::Node(NodeError::PeerConnectionTimeout((local, remote))))
            if local == btcd_alpha.get_p2p_socket() && remote == unavailable_socket
    ));
}

/// Verify that [`BtcD`] connects to all fixed peers during startup.
#[test]
fn btcd_connects_to_fixed_peers() {
    let peer_alpha = BtcD::new().unwrap();
    let peer_beta = BtcD::new().unwrap();
    let peers = [peer_alpha.get_p2p_socket(), peer_beta.get_p2p_socket()];
    let mut config = BtcDConf::default();
    config.args.fixed_peers = peers.to_vec();

    let btcd = BtcD::new_with_conf(&config).unwrap();

    wait_for_fixed_peers(&btcd, &peers, CONNECTION_TIMEOUT);
    assert_eq!(btcd.get_peer_count().unwrap(), peers.len() as u32);
}

/// Verify block propagation from one [`Node`](crate::node::Node) to a peer.
#[test]
fn btcd_blocks_propagate() {
    let btcd_alpha = BtcD::new().unwrap();
    let btcd_beta = BtcD::new().unwrap();

    btcd_alpha.generate(21).unwrap();

    assert_eq!(btcd_alpha.get_chain_tip().unwrap(), 21);
    assert_eq!(btcd_beta.get_chain_tip().unwrap(), 0);

    connect(&btcd_alpha, &btcd_beta).unwrap();

    wait_for_height(&btcd_beta, 21).unwrap();
    assert_eq!(btcd_beta.get_chain_tip().unwrap(), 21);

    btcd_beta.generate(21).unwrap();
    wait_for_height(&btcd_alpha, 42).unwrap();
    assert_eq!(btcd_alpha.get_chain_tip().unwrap(), 42);
}

/// Verify that `config` contains an invalid typed configuration.
fn assert_invalid(config: &BtcDConf) {
    assert!(matches!(
        BtcD::configured_args(config),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));
}

#[test]
fn btcd_default_configuration_preserves_existing_behavior() {
    let config = BtcDConf::default();

    assert!(config.raw_args.is_empty());
    assert_eq!(config.args.network, Network::Regtest);
    assert!(config.args.fixed_peers.is_empty());
    assert!(config.args.cbf_index);
    assert_eq!(config.args.prune, PruneMode::Disabled);
    assert!(config.args.v2_transport);
    assert!(config.args.txindex);
    assert!(!config.btcd_args.dns_seed);
    assert_eq!(
        config
            .btcd_args
            .mining_address
            .as_ref()
            .unwrap()
            .assume_checked_ref()
            .to_string(),
        DEFAULT_MINING_ADDRESS
    );
    assert_eq!(
        BtcD::configured_args(&config).unwrap(),
        [
            "--regtest",
            "--prune=0",
            "--v2transport",
            "--txindex",
            "--notls",
            "--nodnsseed",
            "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
        ]
    );
}

#[test]
fn btcd_renders_fixed_peers() {
    let mut config = BtcDConf::default();
    config.args.fixed_peers = ["127.0.0.1:18444", "[::1]:18445"]
        .map(|peer| peer.parse().unwrap())
        .to_vec();

    let args = BtcD::configured_args(&config).unwrap();

    assert!(args.contains(&"--connect=127.0.0.1:18444".to_string()));
    assert!(args.contains(&"--connect=[::1]:18445".to_string()));
}

#[test]
fn btcd_renders_supported_networks() {
    let cases = [
        (Network::Bitcoin, None),
        (Network::Testnet, Some("--testnet")),
        (Network::Testnet4, Some("--testnet4")),
        (Network::Signet, Some("--signet")),
        (Network::Regtest, Some("--regtest")),
    ];

    for (network, switch) in cases {
        let mut config = BtcDConf::default();
        config.args.network = network;
        config.btcd_args.mining_address = None;
        let args = BtcD::configured_args(&config).unwrap();
        match switch {
            Some(switch) => assert!(args.contains(&switch.to_string())),
            None => {
                assert!(!args.iter().any(|arg| {
                    ["--testnet", "--testnet4", "--signet", "--regtest"].contains(&arg.as_str())
                }));
            }
        }
    }
}

#[test]
fn btcd_renders_boolean_and_daemon_specific_flags() {
    let mut config = BtcDConf::default();
    config.args.cbf_index = false;
    config.args.v2_transport = false;
    config.args.txindex = false;
    config.btcd_args.dns_seed = true;
    config.btcd_args.mining_address = None;

    let args = BtcD::configured_args(&config).unwrap();
    assert!(args.contains(&"--nocfilters".to_string()));
    assert!(!args.contains(&"--v2transport".to_string()));
    assert!(!args.contains(&"--txindex".to_string()));
    assert!(!args.contains(&"--nodnsseed".to_string()));
    assert!(!args.iter().any(|arg| arg.starts_with("--miningaddr=")));
    assert!(args.contains(&"--notls".to_string()));
}

#[test]
fn btcd_validates_pruning_modes() {
    let mut config = BtcDConf::default();
    config.args.txindex = false;
    config.args.prune = PruneMode::Automatic(1_536);
    assert!(
        BtcD::configured_args(&config)
            .unwrap()
            .contains(&"--prune=1536".to_string())
    );

    config.args.prune = PruneMode::Automatic(1_535);
    assert_invalid(&config);

    config.args.prune = PruneMode::Manual;
    assert_invalid(&config);

    config.args.prune = PruneMode::Automatic(1_536);
    config.args.txindex = true;
    assert_invalid(&config);
}

#[test]
fn btcd_validates_mining_address_network() {
    let mut config = BtcDConf::default();
    config.args.network = Network::Bitcoin;
    assert_invalid(&config);

    config.btcd_args.mining_address = Some(
        Address::from_str("1BitcoinEaterAddressDontSendf59kuE").expect("valid mainnet address"),
    );
    let args = BtcD::configured_args(&config).unwrap();
    assert!(args.contains(&"--miningaddr=1BitcoinEaterAddressDontSendf59kuE".to_string()));
}

#[test]
fn btcd_rejects_raw_typed_and_invariant_argument_spellings() {
    let conflicts = [
        "--regtest",
        "--noregtest",
        "--testnet=true",
        "--connect=127.0.0.1:18444",
        "--cfilters",
        "--nocfilters",
        "--prune=0",
        "--noprune",
        "--v2transport",
        "--nov2transport",
        "--txindex=true",
        "--notxindex",
        "--dnsseed",
        "--nodnsseed",
        "--generate",
        "--nogenerate",
        "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
        "--configfile=/tmp/btcd.conf",
        "--logdir=/tmp/btcd-logs",
        "--norpc",
        "--notls",
        "--tls",
        "--datadir=/tmp/btcd",
        "--listen=127.0.0.1:18333",
        "--rpcpass=secret",
        "--rpclisten=127.0.0.1:18334",
        "--rpcuser=user",
    ];

    for arg in conflicts {
        let config = BtcDConf {
            raw_args: vec![arg.to_string()],
            ..BtcDConf::default()
        };
        assert!(matches!(
            BtcD::configured_args(&config),
            Err(Error::Node(NodeError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }

    let config = BtcDConf {
        raw_args: vec!["--debuglevel=trace".to_string(), "--maxpeers=8".to_string()],
        ..BtcDConf::default()
    };
    assert!(BtcD::configured_args(&config).is_ok());
}

/// Verify process state, typed RPC access, shutdown, and temporary cleanup.
#[test]
fn btcd_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let config = BtcDConf {
        raw_args: vec!["--debuglevel=info".to_string()],
        ..BtcDConf::default()
    };
    let mut btcd = BtcD::from_bin_with_conf(get_btcd_path().unwrap(), &config).unwrap();
    let directory = btcd.get_working_directory();

    assert!(btcd.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(btcd.get_config(), &config);
    assert!(btcd.get_rpc_socket().ip().is_loopback());
    assert!(btcd.get_p2p_socket().ip().is_loopback());
    assert_ne!(btcd.get_rpc_socket(), btcd.get_p2p_socket());
    assert_eq!(
        fs::read_to_string(directory.join(".cookie")).unwrap(),
        "__cookie__:halfin"
    );
    assert!(directory.join("btcd.conf").is_file());
    assert!(directory.join("logs").is_dir());
    btcd.client.uptime().unwrap();

    assert!(btcd.stop().unwrap().success());
    drop(btcd);
    assert!(!directory.exists());
}

/// Verify that regtest startup resets chain state in a static directory.
#[test]
fn btcd_regtest_resets_chain_state_in_static_directory() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("btcd");
    let config = BtcDConf {
        staticdir: Some(directory.clone()),
        ..BtcDConf::default()
    };

    let mut btcd = BtcD::new_with_conf(&config).unwrap();
    btcd.generate(PERSISTENCE_BLOCK_COUNT).unwrap();
    assert!(btcd.stop().unwrap().success());
    drop(btcd);

    assert!(directory.is_dir());

    let mut btcd = BtcD::new_with_conf(&config).unwrap();
    assert_eq!(btcd.get_chain_tip().unwrap(), 0);

    btcd.generate(1).unwrap();
    assert_eq!(btcd.get_chain_tip().unwrap(), 1);
    assert!(btcd.stop().unwrap().success());
    drop(btcd);
    assert!(directory.is_dir());
}
