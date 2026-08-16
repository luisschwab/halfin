// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`UtreexoD`].

use std::fs;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use corepc_client::bitcoin::Address;
use corepc_client::bitcoin::Network;
use corepc_client::client_sync::Auth;
use corepc_client::client_sync::v17::Client;

use super::DEFAULT_MINING_ADDRESS;
use super::UtreexoD;
use super::UtreexoDConf;
use super::get_utreexod_path;
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
fn utreexod_validates_binary_path_and_start_attempts() {
    let error = UtreexoD::from_bin("utreexod").unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotAbsolute { .. }));

    let root = tempfile::tempdir().unwrap();
    let error = UtreexoD::from_bin(root.path().join("missing-utreexod")).unwrap_err();
    assert!(matches!(error, Error::BinaryPathNotFile { .. }));

    let config = UtreexoDConf {
        max_retries: 0,
        ..UtreexoDConf::default()
    };
    let error = UtreexoD::from_bin_with_conf(get_utreexod_path().unwrap(), &config).unwrap_err();
    assert!(matches!(error, Error::StartupAttemptsExhausted(0)));
}

/// Verify directory, spawn, retry, and client-timeout startup failures.
#[cfg(unix)]
#[test]
fn utreexod_reports_test_program_startup_failures() {
    let (_program_directory, program) = test_program("exit 1", true);
    let config = UtreexoDConf {
        tmpdir: Some(program.clone()),
        max_retries: 1,
        ..UtreexoDConf::default()
    };
    assert!(matches!(
        UtreexoD::from_bin_with_conf(&program, &config),
        Err(Error::Io(_))
    ));

    let (_program_directory, program) = test_program("exit 1", false);
    let config = UtreexoDConf {
        max_retries: 1,
        ..UtreexoDConf::default()
    };
    assert!(matches!(
        UtreexoD::from_bin_with_conf(&program, &config),
        Err(Error::FailedToSpawn(_))
    ));

    let (_program_directory, program) = test_program("exit 1", true);
    let config = UtreexoDConf {
        max_retries: 2,
        ..UtreexoDConf::default()
    };
    assert!(matches!(
        UtreexoD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(2))
    ));

    let (_program_directory, program) = test_program("exec sleep 30", true);
    let config = UtreexoDConf {
        max_retries: 1,
        ..UtreexoDConf::default()
    };
    assert!(matches!(
        UtreexoD::from_bin_with_conf(&program, &config),
        Err(Error::StartupAttemptsExhausted(1))
    ));
}

/// Verify malformed successful RPC results are rejected by typed helpers.
#[test]
fn utreexod_rejects_malformed_rpc_results() {
    let mut utreexod = UtreexoD::new().unwrap();
    let (socket, server) = scripted_json_rpc_server(vec![
        serde_json::json!(0),
        serde_json::json!("not-a-block-hash"),
        serde_json::Value::Null,
        serde_json::json!([0]),
        serde_json::json!(["not-a-block-hash"]),
        serde_json::Value::Null,
    ]);
    utreexod.client = Client::new_with_auth(
        &format!("http://{socket}"),
        Auth::UserPass("user".to_string(), "password".to_string()),
    )
    .unwrap();

    assert!(matches!(
        utreexod.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        utreexod.get_block_hash(0),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        utreexod.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        utreexod.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        utreexod.generate(1),
        Err(Error::UnexpectedResponse(_))
    ));
    assert!(matches!(
        utreexod.get_peer_count(),
        Err(Error::UnexpectedResponse(_))
    ));
    server.join().unwrap();
}

/// Verify [`UtreexoD`] startup, process data, and P2P data.
#[test]
fn utreexod_starts() {
    let path = get_utreexod_path().unwrap();
    let config = UtreexoDConf {
        raw_args: vec!["--debuglevel=info".to_string()],
        ..UtreexoDConf::default()
    };
    let utreexod = UtreexoD::from_bin_with_conf(path, &config).unwrap();

    println!("PID: {}", utreexod.get_pid());
    println!("Working Directory: {:?}", utreexod.get_working_directory());
    println!("P2P Socket: {}", utreexod.get_p2p_socket());
    assert_eq!(utreexod.get_config(), &config);
    assert_eq!(Node::get_config(&utreexod), &config);
    assert_eq!(config.as_ref(), &config.args);
    assert_eq!(UtreexoD::poll_interval(), 2 * crate::POLL_INTERVAL);
    let _ = utreexod.get_rpc_client();
    assert!(matches!(
        UtreexoD::wait_for_client(
            &format!("http://{}", utreexod.get_rpc_socket()),
            &Auth::None,
            Duration::from_millis(250),
        ),
        Err(Error::ClientSetupTimeout)
    ));
}

/// Verify startup with typed transaction index configuration.
#[test]
fn utreexod_starts_with_txindex() {
    let mut config = UtreexoDConf::default();
    config.args.txindex = true;

    let utreexod = UtreexoD::new_with_conf(&config).unwrap();
    assert!(utreexod.get_pid() > 0);
}

/// Verify that `generate` mines the specified number of blocks.
#[test]
fn utreexod_generate() {
    let utreexod = UtreexoD::new().unwrap();

    assert_eq!(utreexod.get_chain_tip().unwrap(), 0);
    assert!(matches!(
        Node::get_chain_tip(&utreexod),
        Err(Error::UnexpectedResponse(_))
    ));
    Node::generate(&utreexod, 10).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 10);
}

#[test]
fn utreexod_get_filter_height() {
    let utreexod = UtreexoD::new().unwrap();

    utreexod.generate(FILTER_BLOCK_COUNT).unwrap();
    wait_for_filter_height(&utreexod, FILTER_BLOCK_COUNT).unwrap();

    assert_eq!(utreexod.get_filter_tip().unwrap(), FILTER_BLOCK_COUNT);
}

/// Verify that [`UtreexoD::get_block_hash`] returns the correct hash at a specified height.
#[test]
fn utreexod_get_block_hash() {
    let utreexod = UtreexoD::new().unwrap();

    let block_hashes = utreexod.generate(10).unwrap();

    assert_eq!(
        Node::get_block_hash(&utreexod, 10).unwrap(),
        *block_hashes.last().unwrap()
    );
}

/// Verify a connection between two [`Node`](crate::node::Node) implementations through
/// [`connect`].
#[test]
fn utreexod_addnode() {
    let utreexod_alpha = UtreexoD::new().unwrap();
    let utreexod_beta = UtreexoD::new().unwrap();

    assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 0);
    assert_eq!(utreexod_beta.get_peer_count().unwrap(), 0);

    connect(&utreexod_alpha, &utreexod_beta).unwrap();

    assert_eq!(utreexod_alpha.get_peer_count().unwrap(), 1);
    assert_eq!(utreexod_beta.get_peer_count().unwrap(), 1);

    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_socket = listener.local_addr().unwrap();
    drop(listener);
    assert!(matches!(
        utreexod_alpha.add_peer(unavailable_socket),
        Err(Error::Node(NodeError::PeerConnectionTimeout((local, remote))))
            if local == utreexod_alpha.get_p2p_socket() && remote == unavailable_socket
    ));
}

/// Verify that [`UtreexoD`] connects to all fixed peers during startup.
#[test]
fn utreexod_connects_to_fixed_peers() {
    let peer_alpha = UtreexoD::new().unwrap();
    let peer_beta = UtreexoD::new().unwrap();
    let peers = [peer_alpha.get_p2p_socket(), peer_beta.get_p2p_socket()];
    let mut config = UtreexoDConf::default();
    config.args.fixed_peers = peers.to_vec();

    let utreexod = UtreexoD::new_with_conf(&config).unwrap();

    wait_for_fixed_peers(&utreexod, &peers, CONNECTION_TIMEOUT);
    assert_eq!(utreexod.get_peer_count().unwrap(), peers.len() as u32);
}

/// Verify block propagation from one [`Node`](crate::node::Node) to a peer.
#[test]
fn utreexod_blocks_propagate() {
    let utreexod_alpha = UtreexoD::new().unwrap();
    let utreexod_beta = UtreexoD::new().unwrap();

    utreexod_alpha.generate(21).unwrap();

    assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 21);
    assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 0);

    connect(&utreexod_alpha, &utreexod_beta).unwrap();

    wait_for_height(&utreexod_beta, 21).unwrap();
    assert_eq!(utreexod_beta.get_chain_tip().unwrap(), 21);

    utreexod_beta.generate(21).unwrap();
    wait_for_height(&utreexod_alpha, 42).unwrap();
    assert_eq!(utreexod_alpha.get_chain_tip().unwrap(), 42);
}

/// Verify that `config` contains an invalid typed configuration.
fn assert_invalid(config: &UtreexoDConf) {
    assert!(matches!(
        UtreexoD::configured_args(config),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));
}

#[test]
fn utreexod_default_configuration_preserves_existing_behavior() {
    let config = UtreexoDConf::default();

    assert!(config.raw_args.is_empty());
    assert_eq!(config.args.network, Network::Regtest);
    assert!(config.args.fixed_peers.is_empty());
    assert!(config.args.cbf_index);
    assert_eq!(config.args.prune, PruneMode::Disabled);
    assert!(config.args.v2_transport);
    assert!(!config.args.txindex);
    assert!(!config.utreexod_args.dns_seed);
    assert!(!config.utreexod_args.assume_utreexo);
    assert_eq!(config.utreexod_args.proof_index_max_memory_mib, 256);
    assert_eq!(
        config
            .utreexod_args
            .mining_address
            .as_ref()
            .unwrap()
            .assume_checked_ref()
            .to_string(),
        DEFAULT_MINING_ADDRESS
    );
    assert_eq!(
        UtreexoD::configured_args(&config).unwrap(),
        [
            "--regtest",
            "--cfilters",
            "--prune=0",
            "--v2transport",
            "--notls",
            "--nodnsseed",
            "--noassumeutreexo",
            "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
            "--flatutreexoproofindex",
            "--utreexoproofindexmaxmemory=256",
        ]
    );
}

#[test]
fn utreexod_renders_fixed_peers() {
    let mut config = UtreexoDConf::default();
    config.args.fixed_peers = ["127.0.0.1:18444", "[::1]:18445"]
        .map(|peer| peer.parse().unwrap())
        .to_vec();

    let args = UtreexoD::configured_args(&config).unwrap();

    assert!(args.contains(&"--connect=127.0.0.1:18444".to_string()));
    assert!(args.contains(&"--connect=[::1]:18445".to_string()));
}

#[test]
fn utreexod_renders_supported_networks_and_data_paths() {
    let cases = [
        (Network::Bitcoin, None, "mainnet"),
        (Network::Testnet, Some("--testnet"), "testnet3"),
        (Network::Signet, Some("--signet"), "signet"),
        (Network::Regtest, Some("--regtest"), "regtest"),
    ];

    for (network, switch, data_directory) in cases {
        let mut config = UtreexoDConf::default();
        config.args.network = network;
        config.utreexod_args.mining_address = None;
        let args = UtreexoD::configured_args(&config).unwrap();
        match switch {
            Some(switch) => assert!(args.contains(&switch.to_string())),
            None => {
                assert!(
                    !args.iter().any(|arg| {
                        ["--testnet", "--signet", "--regtest"].contains(&arg.as_str())
                    })
                );
            }
        }
        assert_eq!(UtreexoD::network_data_dir_name(network), data_directory);
    }

    assert_eq!(
        UtreexoD::network_data_dir_name(Network::Testnet4),
        "testnet4"
    );
}

#[test]
fn utreexod_rejects_testnet4() {
    let mut config = UtreexoDConf::default();
    config.args.network = Network::Testnet4;
    config.utreexod_args.mining_address = None;
    assert_invalid(&config);
}

#[test]
fn utreexod_renders_boolean_and_daemon_specific_flags() {
    let mut config = UtreexoDConf::default();
    config.args.cbf_index = false;
    config.args.v2_transport = false;
    config.args.txindex = true;
    config.utreexod_args.dns_seed = true;
    config.utreexod_args.assume_utreexo = true;
    config.utreexod_args.mining_address = None;
    config.utreexod_args.proof_index_max_memory_mib = 512;

    let args = UtreexoD::configured_args(&config).unwrap();
    assert!(!args.contains(&"--cfilters".to_string()));
    assert!(!args.contains(&"--v2transport".to_string()));
    assert!(args.contains(&"--txindex".to_string()));
    assert!(!args.contains(&"--nodnsseed".to_string()));
    assert!(!args.contains(&"--noassumeutreexo".to_string()));
    assert!(!args.iter().any(|arg| arg.starts_with("--miningaddr=")));
    assert!(args.contains(&"--notls".to_string()));
    assert!(args.contains(&"--flatutreexoproofindex".to_string()));
    assert!(args.contains(&"--utreexoproofindexmaxmemory=512".to_string()));
}

#[test]
fn utreexod_validates_pruning_modes() {
    let mut config = UtreexoDConf::default();
    config.args.prune = PruneMode::Automatic(550);
    assert!(
        UtreexoD::configured_args(&config)
            .unwrap()
            .contains(&"--prune=550".to_string())
    );

    config.args.prune = PruneMode::Automatic(549);
    assert_invalid(&config);

    config.args.prune = PruneMode::Manual;
    assert_invalid(&config);

    config.args.prune = PruneMode::Automatic(550);
    config.args.txindex = true;
    assert_invalid(&config);
}

#[test]
fn utreexod_validates_proof_index_memory() {
    let mut config = UtreexoDConf::default();
    config.utreexod_args.proof_index_max_memory_mib = 249;
    assert_invalid(&config);

    config.utreexod_args.proof_index_max_memory_mib = 250;
    assert!(UtreexoD::configured_args(&config).is_ok());
}

#[test]
fn utreexod_validates_mining_address_network() {
    let mut config = UtreexoDConf::default();
    config.args.network = Network::Bitcoin;
    assert_invalid(&config);

    config.utreexod_args.mining_address = Some(
        Address::from_str("1BitcoinEaterAddressDontSendf59kuE").expect("valid mainnet address"),
    );
    let args = UtreexoD::configured_args(&config).unwrap();
    assert!(args.contains(&"--miningaddr=1BitcoinEaterAddressDontSendf59kuE".to_string()));
}

#[test]
fn utreexod_rejects_raw_typed_and_invariant_argument_spellings() {
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
        "--assumeutreexo",
        "--noassumeutreexo",
        "--miningaddr=bcrt1qusgerygumpd0ztn735s5pypq6wsv2zzhuc4yak",
        "--notls",
        "--tls",
        "--flatutreexoproofindex",
        "--noflatutreexoproofindex",
        "--utreexoproofindex",
        "--utreexoproofindexmaxmemory=500",
        "--datadir=/tmp/utreexo",
        "--listen=127.0.0.1:18333",
        "--rpcpass=secret",
        "--rpclisten=127.0.0.1:18334",
        "--rpcuser=user",
    ];

    for arg in conflicts {
        let config = UtreexoDConf {
            raw_args: vec![arg.to_string()],
            ..UtreexoDConf::default()
        };
        assert!(matches!(
            UtreexoD::configured_args(&config),
            Err(Error::Node(NodeError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }

    let config = UtreexoDConf {
        raw_args: vec!["--debuglevel=trace".to_string(), "--maxpeers=8".to_string()],
        ..UtreexoDConf::default()
    };
    assert!(UtreexoD::configured_args(&config).is_ok());
}

/// Verify process state, typed RPC access, shutdown, and temporary cleanup.
#[test]
fn utreexod_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let config = UtreexoDConf {
        raw_args: vec!["--debuglevel=info".to_string()],
        ..UtreexoDConf::default()
    };
    let mut utreexod = UtreexoD::from_bin_with_conf(get_utreexod_path().unwrap(), &config).unwrap();
    let directory = utreexod.get_working_directory();

    assert!(utreexod.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(utreexod.get_config(), &config);
    assert!(utreexod.get_rpc_socket().ip().is_loopback());
    assert!(utreexod.get_p2p_socket().ip().is_loopback());
    assert_ne!(utreexod.get_rpc_socket(), utreexod.get_p2p_socket());
    assert_eq!(
        fs::read_to_string(directory.join(".cookie")).unwrap(),
        "__cookie__:halfin"
    );
    utreexod.client.uptime().unwrap();

    assert!(utreexod.stop().unwrap().success());
    drop(utreexod);
    assert!(!directory.exists());
}

/// Verify that regtest startup resets chain state in a static directory.
#[test]
fn utreexod_regtest_resets_chain_state_in_static_directory() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("utreexod");
    let config = UtreexoDConf {
        staticdir: Some(directory.clone()),
        ..UtreexoDConf::default()
    };

    let mut utreexod = UtreexoD::new_with_conf(&config).unwrap();
    utreexod.generate(PERSISTENCE_BLOCK_COUNT).unwrap();
    // Give the forest writer time to flush before a clean RPC shutdown.
    sleep(Duration::from_secs(2));
    assert!(utreexod.stop().unwrap().success());
    drop(utreexod);

    assert!(directory.is_dir());

    let mut utreexod = UtreexoD::new_with_conf(&config).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 0);

    utreexod.generate(1).unwrap();
    assert_eq!(utreexod.get_chain_tip().unwrap(), 1);
    assert!(utreexod.stop().unwrap().success());
    drop(utreexod);
    assert!(directory.is_dir());
}
