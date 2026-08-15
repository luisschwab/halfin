// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`FlorestaD`].

#[cfg(feature = "utreexod")]
use core::time::Duration;

use corepc_client::bitcoin::Network;
use electrum_client::ElectrumApi;

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
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;
#[cfg(feature = "utreexod")]
use crate::node::wait_for_height_with_timeout;

#[cfg(feature = "utreexod")]
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Verify Floresta startup, process data, and RPC data.
#[test]
fn florestad_starts() {
    let config = FlorestaDConf {
        raw_args: vec!["--debug".to_string()],
        ..FlorestaDConf::default()
    };
    let florestad = FlorestaD::from_bin_with_conf(get_florestad_path().unwrap(), &config).unwrap();

    assert!(florestad.get_pid() > 0);
    assert!(florestad.get_working_directory().is_dir());
    assert_eq!(florestad.get_config(), &config);
    assert_eq!(florestad.get_chain_tip().unwrap(), 0);
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

#[test]
fn florestad_default_configuration_is_isolated_regtest() {
    let config = FlorestaDConf::default();

    assert_eq!(config.args.network, Network::Regtest);
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
