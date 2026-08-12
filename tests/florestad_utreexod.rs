// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for [`FlorestaD`] backed by [`UtreexoD`].

#![cfg(all(feature = "florestad", feature = "utreexod"))]

use std::time::Duration;

use electrum_client::ElectrumApi;
use halfin::Error;
use halfin::bitcoin::Address;
use halfin::bitcoin::Block;
use halfin::bitcoin::Network;
use halfin::bitcoin::consensus::deserialize;
use halfin::bitcoin::hex::FromHex;
use halfin::florestad::FlorestaD;
use halfin::florestad::FlorestaDConf;
use halfin::florestad::get_florestad_path;
use halfin::node::Node;
use halfin::node::connect_and_sync;
use halfin::node::wait_for_height_with_timeout;
use halfin::utreexod::UtreexoD;
use halfin::utreexod::UtreexoDConf;
use miniscript::Descriptor;
use miniscript::DescriptorPublicKey;

const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Verify that Floresta starts and exposes its process and RPC state.
#[test]
fn test_florestad_starts() {
    let conf = FlorestaDConf {
        raw_args: vec!["--debug".to_string()],
        ..FlorestaDConf::default()
    };
    let florestad = FlorestaD::from_bin_with_conf(get_florestad_path().unwrap(), &conf).unwrap();

    assert!(florestad.get_pid() > 0);
    assert!(florestad.get_working_directory().is_dir());
    assert_eq!(florestad.get_config(), &conf);
    assert_eq!(florestad.get_chain_tip().unwrap(), 0);
    assert_eq!(florestad.get_peer_count().unwrap(), 0);
    assert_eq!(
        florestad.get_electrum_url(),
        florestad.get_electrum_socket().to_string()
    );
    florestad.get_electrum_client().ping().unwrap();
    assert!(matches!(
        florestad.generate(1),
        Err(Error::UnsupportedCommand {
            node: "FlorestaD",
            command: "generate"
        })
    ));
    assert!(matches!(
        florestad.get_filter_tip(),
        Err(Error::UnsupportedCommand {
            node: "FlorestaD",
            command: "get_filter_tip"
        })
    ));
}

/// Verify that Floresta synchronizes a chain mined by `utreexod`.
#[test]
fn test_florestad_syncs_from_utreexod() {
    const HISTORICAL_BLOCKS: u32 = 6;
    const LIVE_BLOCKS: u32 = 4;

    let utreexod = UtreexoD::new().unwrap();
    let mut expected_hashes = utreexod.generate(HISTORICAL_BLOCKS).unwrap();

    let florestad = FlorestaD::new().unwrap();
    let utreexod_socket = utreexod.get_p2p_socket();

    assert_eq!(florestad.get_chain_tip().unwrap(), 0);
    connect_and_sync(&florestad, &utreexod).unwrap();
    assert!(florestad.has_peer(utreexod_socket).unwrap());
    assert_eq!(florestad.get_peer_count().unwrap(), 1);

    expected_hashes.extend(utreexod.generate(LIVE_BLOCKS).unwrap());
    let expected_height = HISTORICAL_BLOCKS + LIVE_BLOCKS;
    wait_for_height_with_timeout(&florestad, expected_height, SYNC_TIMEOUT).unwrap();

    assert_eq!(florestad.get_chain_tip().unwrap(), expected_height);
    for (height, expected_hash) in (1..=expected_height).zip(&expected_hashes) {
        assert_eq!(florestad.get_block_hash(height).unwrap(), *expected_hash);
    }

    let blockchain_info = florestad.call("getblockchaininfo", &[]).unwrap();
    let expected_tip = expected_hashes.last().unwrap().to_string();
    assert_eq!(blockchain_info["chain"].as_str(), Some("regtest"));
    assert_eq!(
        blockchain_info["height"].as_u64(),
        Some(u64::from(expected_height))
    );
    assert_eq!(
        blockchain_info["best_block"].as_str(),
        Some(expected_tip.as_str())
    );

    let electrum_tip = florestad
        .get_electrum_client()
        .block_headers_subscribe()
        .unwrap();
    assert_eq!(electrum_tip.height as u32, expected_height);
    assert_eq!(
        electrum_tip.header.block_hash(),
        florestad.get_block_hash(expected_height).unwrap()
    );
    assert!(florestad.call("uptime", &[]).unwrap().is_number());
}

/// Verify that Floresta loads a descriptor and syncs blocks paying its script.
#[test]
fn test_florestad_loads_wallet_descriptor() {
    const BLOCK_COUNT: u32 = 3;
    const PUBLIC_KEY: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    let descriptor: Descriptor<DescriptorPublicKey> =
        format!("wpkh({PUBLIC_KEY})").parse().unwrap();
    let descriptor_string = descriptor.to_string();
    let script_pubkey = descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let mining_address = Address::from_script(&script_pubkey, Network::Regtest)
        .unwrap()
        .into_unchecked();

    let mut utreexod_conf = UtreexoDConf::default();
    utreexod_conf.utreexod_args.mining_address = Some(mining_address);
    let utreexod = UtreexoD::new_with_conf(&utreexod_conf).unwrap();
    let expected_hashes = utreexod.generate(BLOCK_COUNT).unwrap();

    let mut florestad_conf = FlorestaDConf::default();
    florestad_conf
        .florestad_args
        .wallet_descriptors
        .push(descriptor);
    let florestad = FlorestaD::new_with_conf(&florestad_conf).unwrap();
    connect_and_sync(&florestad, &utreexod).unwrap();

    assert_eq!(
        florestad.call("listdescriptors", &[]).unwrap(),
        serde_json::json!([descriptor_string])
    );
    for (height, expected_hash) in (1..=BLOCK_COUNT).zip(expected_hashes) {
        assert_eq!(florestad.get_block_hash(height).unwrap(), expected_hash);

        let block_hex = florestad
            .call("getblock", &[expected_hash.to_string().into(), 0.into()])
            .unwrap();
        let block_hex = block_hex.as_str().unwrap();
        let block: Block = deserialize(&Vec::<u8>::from_hex(block_hex).unwrap()).unwrap();
        assert!(
            block
                .txdata
                .iter()
                .flat_map(|transaction| &transaction.output)
                .any(|output| output.script_pubkey == script_pubkey),
            "block {height} does not pay the configured descriptor"
        );
    }
}
