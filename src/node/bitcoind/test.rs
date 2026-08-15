// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration and runtime integration tests for [`BitcoinD`].

use std::fs;

use corepc_client::bitcoin::Amount;
use corepc_client::bitcoin::FeeRate;
use corepc_client::bitcoin::Network;

use super::BitcoinD;
use super::BitcoinDConf;
use super::get_bitcoind_path;
use crate::Error;
use crate::FILTER_BLOCK_COUNT;
use crate::MATURE_COINBASE_BLOCK_COUNT;
use crate::PERSISTENCE_BLOCK_COUNT;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::node::connect;
use crate::node::wait_for_filter_height;
use crate::node::wait_for_height;

/// Verify [`BitcoinD`] startup, process data, and P2P data.
#[test]
fn bitcoind_starts() {
    let bin = get_bitcoind_path().unwrap();
    let conf = BitcoinDConf {
        raw_args: vec!["-debug=net".to_string()],
        ..BitcoinDConf::default()
    };
    let bitcoind = BitcoinD::from_bin_with_conf(bin, &conf).unwrap();

    println!("PID: {}", bitcoind.get_pid());
    println!("Working Directory: {:?}", bitcoind.get_working_directory());
    println!("P2P Socket: {}", bitcoind.get_p2p_socket());
    assert_eq!(bitcoind.get_config(), &conf);
}

/// Verify that `generate` mines the specified number of blocks.
#[test]
fn bitcoind_generate() {
    let bitcoind = BitcoinD::new().unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    assert_eq!(height, 0);

    bitcoind.generate(10).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    assert_eq!(height, 10);
}

/// Verify that `generatetoaddress` mines the specified number of blocks to an address.
#[test]
fn bitcoind_generate_to_address() {
    const GENERATED_BLOCK_COUNT: u32 = 21;

    let bitcoind = BitcoinD::new().unwrap();

    let address = bitcoind
        .client
        .get_new_address(None, None)
        .unwrap()
        .address()
        .unwrap()
        .assume_checked();

    bitcoind
        .generate_to_address(GENERATED_BLOCK_COUNT, &address)
        .unwrap();

    let address_desc = format!("addr({})", address);
    let address_balance = bitcoind
        .client
        .scan_tx_out_set_start(&[&address_desc])
        .unwrap()
        .total_amount;

    assert_eq!(
        Amount::from_btc(address_balance).unwrap(),
        Amount::from_int_btc(u64::from(GENERATED_BLOCK_COUNT) * 50)
    );
}

#[test]
fn bitcoind_get_filter_height() {
    let bitcoind = BitcoinD::new().unwrap();

    bitcoind.generate(FILTER_BLOCK_COUNT).unwrap();
    wait_for_filter_height(&bitcoind, FILTER_BLOCK_COUNT).unwrap();

    assert_eq!(FILTER_BLOCK_COUNT, bitcoind.get_filter_tip().unwrap());
}

/// Verify that [`BitcoinD::get_block_hash`] returns the correct block hash for a specified height.
#[test]
fn bitcoind_get_block_hash() {
    let bitcoind = BitcoinD::new().unwrap();

    let block_hashes = bitcoind.generate(10).unwrap();

    let last_block_hash = bitcoind.get_block_hash(10).unwrap();

    assert_eq!(last_block_hash, *block_hashes.last().unwrap());
}

/// Verify a connection between two [`Node`](crate::node::Node) implementations through
/// [`connect`].
/// Verify that both peer counts include the new connection.
#[test]
fn bitcoind_addnode() {
    let bitcoind_alpha = BitcoinD::new().unwrap();
    let bitcoind_beta = BitcoinD::new().unwrap();

    assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 0);
    assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 0);

    connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

    assert_eq!(bitcoind_alpha.get_peer_count().unwrap(), 1);
    assert_eq!(bitcoind_beta.get_peer_count().unwrap(), 1);
}

/// Verify block propagation from one [`Node`](crate::node::Node) to a peer.
#[test]
fn bitcoind_blocks_propagate() {
    let bitcoind_alpha = BitcoinD::new().unwrap();
    let bitcoind_beta = BitcoinD::new().unwrap();

    bitcoind_alpha.generate(21).unwrap();

    assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 21);
    assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 0);

    connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

    wait_for_height(&bitcoind_beta, 21).unwrap();
    assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 21);

    bitcoind_beta.generate(21).unwrap();
    wait_for_height(&bitcoind_alpha, 42).unwrap();
    assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 42);
}

/// Verify that `conf` contains an invalid typed configuration.
fn assert_invalid(conf: &BitcoinDConf) {
    assert!(matches!(
        BitcoinD::configured_args(conf),
        Err(Error::Node(NodeError::InvalidConfiguration(_)))
    ));
}

#[test]
fn bitcoind_default_configuration_preserves_existing_behavior() {
    let conf = BitcoinDConf::default();

    assert!(conf.raw_args.is_empty());
    assert_eq!(conf.args.network, Network::Regtest);
    assert!(conf.args.cbf_index);
    assert_eq!(conf.args.prune, PruneMode::Disabled);
    assert!(conf.args.v2_transport);
    assert!(conf.args.txindex);
    assert_eq!(
        conf.bitcoind_args.fallback_fee_rate,
        FeeRate::from_sat_per_vb_u32(10)
    );
    assert_eq!(
        BitcoinD::configured_args(&conf).unwrap(),
        [
            "-chain=regtest",
            "-blockfilterindex=1",
            "-prune=0",
            "-v2transport=1",
            "-txindex=1",
            "-fallbackfee=0.0001",
        ]
    );
}

#[test]
fn bitcoind_renders_all_networks() {
    let cases = [
        (Network::Bitcoin, "main"),
        (Network::Testnet, "test"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ];

    for (network, core_arg) in cases {
        let mut conf = BitcoinDConf::default();
        conf.args.network = network;
        let args = BitcoinD::configured_args(&conf).unwrap();
        assert_eq!(args[0], format!("-chain={core_arg}"));
    }
}

#[test]
fn bitcoind_renders_boolean_and_pruning_flags() {
    let mut conf = BitcoinDConf::default();
    conf.args.cbf_index = false;
    conf.args.v2_transport = false;
    conf.args.txindex = false;
    conf.args.prune = PruneMode::Manual;
    let args = BitcoinD::configured_args(&conf).unwrap();
    assert!(args.contains(&"-blockfilterindex=0".to_string()));
    assert!(args.contains(&"-prune=1".to_string()));
    assert!(args.contains(&"-v2transport=0".to_string()));
    assert!(args.contains(&"-txindex=0".to_string()));

    conf.args.prune = PruneMode::Automatic(550);
    let args = BitcoinD::configured_args(&conf).unwrap();
    assert!(args.contains(&"-prune=550".to_string()));

    conf.args.prune = PruneMode::Automatic(549);
    assert_invalid(&conf);
}

#[test]
fn bitcoind_rejects_pruning_with_txindex() {
    let mut conf = BitcoinDConf::default();
    conf.args.prune = PruneMode::Automatic(550);
    assert_invalid(&conf);
}

#[test]
fn bitcoind_formats_fallback_fee_with_bitcoin_amount() {
    let cases = [
        (FeeRate::from_sat_per_vb_u32(10), "-fallbackfee=0.0001"),
        (FeeRate::from_sat_per_kwu(1), "-fallbackfee=0.00000004"),
        (FeeRate::ZERO, "-fallbackfee=0"),
        (FeeRate::from_sat_per_kwu(25_000_000), "-fallbackfee=1"),
    ];

    for (fee_rate, expected) in cases {
        let mut conf = BitcoinDConf::default();
        conf.bitcoind_args.fallback_fee_rate = fee_rate;
        assert!(
            BitcoinD::configured_args(&conf)
                .unwrap()
                .contains(&expected.to_string())
        );
    }

    let mut conf = BitcoinDConf::default();
    conf.bitcoind_args.fallback_fee_rate = FeeRate::MAX;
    assert_invalid(&conf);
}

#[test]
fn bitcoind_rejects_raw_typed_argument_spellings() {
    let conflicts = [
        "-chain=signet",
        "--regtest",
        "-noregtest",
        "-blockfilterindex=0",
        "--blockfilterindex",
        "-noblockfilterindex",
        "--no-blockfilterindex",
        "-prune=550",
        "-noprune",
        "-v2transport=0",
        "-nov2transport",
        "-txindex",
        "--txindex=1",
        "-notxindex",
        "-fallbackfee=0.1",
        "-bind=127.0.0.1:18444",
        "-listen=0",
        "-port=18444",
        "-datadir=/tmp/bitcoin",
        "-rpcbind=127.0.0.1",
        "-rpcpassword=secret",
        "-rpcport=18443",
        "-rpcuser=user",
    ];

    for arg in conflicts {
        let conf = BitcoinDConf {
            raw_args: vec![arg.to_string()],
            ..BitcoinDConf::default()
        };
        assert!(matches!(
            BitcoinD::configured_args(&conf),
            Err(Error::Node(NodeError::ConflictingArgument(conflict))) if conflict == arg
        ));
    }

    let conf = BitcoinDConf {
        raw_args: vec!["-debug=net".to_string(), "-maxconnections=8".to_string()],
        ..BitcoinDConf::default()
    };
    assert!(BitcoinD::configured_args(&conf).is_ok());
}

/// Verify process state, RPC access, authentication, shutdown, and temporary cleanup.
#[test]
fn bitcoind_lifecycle_exposes_runtime_state_and_removes_temporary_directory() {
    let conf = BitcoinDConf {
        raw_args: vec!["-debug=net".to_string()],
        ..BitcoinDConf::default()
    };
    let mut bitcoind = BitcoinD::from_bin_with_conf(get_bitcoind_path().unwrap(), &conf).unwrap();
    let directory = bitcoind.get_working_directory();

    assert!(bitcoind.get_pid() > 0);
    assert!(directory.is_dir());
    assert_eq!(bitcoind.get_config(), &conf);
    assert!(bitcoind.get_rpc_socket().ip().is_loopback());
    assert!(bitcoind.get_p2p_socket().ip().is_loopback());
    assert_ne!(bitcoind.get_rpc_socket(), bitcoind.get_p2p_socket());
    assert_eq!(bitcoind.get_cookie_file(), directory.join(".cookie"));
    assert_eq!(
        fs::read_to_string(bitcoind.get_cookie_file()).unwrap(),
        "__cookie__:halfin"
    );
    bitcoind.client.uptime().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(bitcoind.get_cookie_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    assert!(bitcoind.stop().unwrap().success());
    drop(bitcoind);
    assert!(!directory.exists());
}

/// Verify mining, wallet payments, compact filters, and tip replacement.
#[test]
fn bitcoind_chain_wallet_and_reorganization_operations_work_together() {
    let bitcoind = BitcoinD::new().unwrap();
    assert_eq!(bitcoind.get_chain_tip().unwrap(), 0);

    let block_hashes = bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();
    assert_eq!(block_hashes.len(), MATURE_COINBASE_BLOCK_COUNT as usize);
    assert_eq!(
        bitcoind.get_chain_tip().unwrap(),
        MATURE_COINBASE_BLOCK_COUNT
    );
    assert_eq!(
        bitcoind
            .get_block_hash(MATURE_COINBASE_BLOCK_COUNT)
            .unwrap(),
        *block_hashes.last().unwrap()
    );
    wait_for_filter_height(&bitcoind, MATURE_COINBASE_BLOCK_COUNT).unwrap();
    assert_eq!(
        bitcoind.get_filter_tip().unwrap(),
        MATURE_COINBASE_BLOCK_COUNT
    );
    assert!(bitcoind.client.get_balance().unwrap().balance().unwrap() >= Amount::from_int_btc(50));

    let address = bitcoind.client.new_address().unwrap();
    let amount = Amount::from_int_btc(1);
    let txid = bitcoind
        .client
        .send_to_address(&address, amount)
        .unwrap()
        .txid()
        .unwrap();
    let mempool = bitcoind
        .client
        .get_raw_mempool()
        .unwrap()
        .into_model()
        .unwrap();
    assert!(mempool.0.contains(&txid));

    let coinbase_address = bitcoind.client.new_address().unwrap();
    let block_hash = bitcoind
        .generate_to_address(1, &coinbase_address)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let height = MATURE_COINBASE_BLOCK_COUNT + 1;
    assert_eq!(bitcoind.get_chain_tip().unwrap(), height);
    assert_eq!(bitcoind.get_block_hash(height).unwrap(), block_hash);

    let descriptor = format!("addr({address})");
    let outputs = bitcoind
        .client
        .scan_tx_out_set_start(&[&descriptor])
        .unwrap()
        .into_model()
        .unwrap();
    assert_eq!(outputs.total_amount, amount);

    bitcoind.invalidate_blocks(1).unwrap();
    assert_eq!(
        bitcoind.get_chain_tip().unwrap(),
        MATURE_COINBASE_BLOCK_COUNT
    );
    let mempool = bitcoind
        .client
        .get_raw_mempool()
        .unwrap()
        .into_model()
        .unwrap();
    assert!(mempool.0.contains(&txid));

    let address = bitcoind.client.new_address().unwrap();
    let replacement_hash = bitcoind
        .generate_to_address(1, &address)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_ne!(replacement_hash, block_hash);
    assert_eq!(bitcoind.get_chain_tip().unwrap(), height);
    assert_eq!(bitcoind.get_block_hash(height).unwrap(), replacement_hash);
}

/// Verify that a static directory retains chain and wallet state across a restart.
#[test]
fn bitcoind_static_directory_restores_chain_and_wallet_state() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let directory = temporary_directory.path().join("bitcoind");
    let conf = BitcoinDConf {
        staticdir: Some(directory.clone()),
        ..BitcoinDConf::default()
    };

    let mut bitcoind = BitcoinD::new_with_conf(&conf).unwrap();
    let address = bitcoind.client.new_address().unwrap();
    let block_hashes = bitcoind.generate(PERSISTENCE_BLOCK_COUNT).unwrap();
    let block_hash = *block_hashes.last().unwrap();
    assert!(bitcoind.stop().unwrap().success());
    drop(bitcoind);

    assert!(directory.is_dir());

    let mut bitcoind = BitcoinD::new_with_conf(&conf).unwrap();
    assert_eq!(bitcoind.get_chain_tip().unwrap(), PERSISTENCE_BLOCK_COUNT);
    assert_eq!(
        bitcoind.get_block_hash(PERSISTENCE_BLOCK_COUNT).unwrap(),
        block_hash
    );
    assert!(bitcoind.client.get_address_info(&address).unwrap().is_mine);

    bitcoind.generate(1).unwrap();
    assert_eq!(
        bitcoind.get_chain_tip().unwrap(),
        PERSISTENCE_BLOCK_COUNT + 1
    );
    assert!(bitcoind.stop().unwrap().success());
    drop(bitcoind);
    assert!(directory.is_dir());
}
