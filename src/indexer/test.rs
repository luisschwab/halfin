// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared integration tests for [`Indexer`] implementations.
//!
//! These tests apply the [`Indexer`] interface to each enabled implementation.
//!
//! [`Indexer`]: crate::indexer::Indexer

#[cfg(feature = "bitcoind")]
use core::fmt::Debug;
use core::time::Duration;
use std::cell::Cell;
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use std::sync::Condvar;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use std::sync::Mutex;
use std::thread::JoinHandle;

#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Amount;
use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Script;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::ScriptBuf;
#[cfg(feature = "bitcoind")]
use corepc_client::bitcoin::Txid;
#[cfg(feature = "bitcoind")]
use electrum_client::ElectrumApi;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use tempfile::TempDir;

#[cfg(feature = "bitcoind")]
use super::Indexer;
use super::ensure_backend_ready;
use super::read_backend_cookie;
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
use crate::CONFIRMATION_BLOCK_COUNT;
use crate::Error;
#[cfg(feature = "bitcoind")]
use crate::MATURE_COINBASE_BLOCK_COUNT;
use crate::indexer::IndexerError;
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
use crate::indexer::electrsd::ElectrsD;
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
use crate::indexer::electrsd::ElectrsDConf;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use crate::indexer::electrumxd::ElectrumxD;
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
use crate::indexer::electrumxd::ElectrumxDConf;
use crate::node::Node;
use crate::node::NodeArgs;
use crate::node::PruneMode;
use crate::node::RPC_COOKIE_FILE_NAME;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;

/// Configuration for [`FakeNode`].
#[derive(Debug)]
pub(super) struct FakeNodeConfig(NodeArgs);

impl AsRef<NodeArgs> for FakeNodeConfig {
    fn as_ref(&self) -> &NodeArgs {
        &self.0
    }
}

/// Deterministic backing node for indexer preparation tests.
#[derive(Debug)]
pub(super) struct FakeNode {
    directory: TempDir,
    config: FakeNodeConfig,
    blockchain_info: serde_json::Value,
    generated_blocks: Cell<u32>,
    fail_blockchain_info: bool,
    fail_generation: bool,
}

impl FakeNode {
    /// Create a backing node with the specified network and blockchain information.
    pub(super) fn new(network: Network, blockchain_info: serde_json::Value) -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
            config: FakeNodeConfig(NodeArgs {
                network,
                fixed_peers: Vec::new(),
                v2_transport: false,
                cbf_index: false,
                prune: PruneMode::Disabled,
                txindex: true,
            }),
            blockchain_info,
            generated_blocks: Cell::new(0),
            fail_blockchain_info: false,
            fail_generation: false,
        }
    }

    /// Configure a backing node that rejects blockchain-information requests.
    pub(super) fn with_blockchain_info_error(mut self) -> Self {
        self.fail_blockchain_info = true;
        self
    }

    /// Configure a backing node that rejects block generation.
    pub(super) fn with_generation_error(mut self) -> Self {
        self.fail_generation = true;
        self
    }

    /// Select the pruning mode exposed to an indexer constructor.
    pub(super) fn with_prune(mut self, prune: PruneMode) -> Self {
        self.config.0.prune = prune;
        self
    }

    /// Select whether the backing node exposes transaction indexing.
    pub(super) fn with_txindex(mut self, txindex: bool) -> Self {
        self.config.0.txindex = txindex;
        self
    }

    /// Write RPC credentials for this backing node.
    pub(super) fn write_cookie(&self, credentials: &str) {
        fs::write(
            self.directory.path().join(RPC_COOKIE_FILE_NAME),
            credentials,
        )
        .unwrap();
    }

    /// Return the number of blocks requested through [`Node::generate`].
    fn generated_blocks(&self) -> u32 {
        self.generated_blocks.get()
    }
}

impl Node for FakeNode {
    type Config = FakeNodeConfig;

    fn get_name() -> &'static str {
        "FakeNode"
    }

    fn get_bin_name() -> &'static str {
        "stub-node"
    }

    fn get_config(&self) -> &Self::Config {
        &self.config
    }

    fn get_working_directory(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }

    fn get_rpc_socket(&self) -> core::net::SocketAddr {
        core::net::SocketAddr::from(([127, 0, 0, 1], 18_443))
    }

    fn generate(&self, count: u32) -> Result<Vec<BlockHash>, Error> {
        if self.fail_generation {
            return Err(Error::UnexpectedResponse(
                "block generation failed".to_string(),
            ));
        }
        self.generated_blocks
            .set(self.generated_blocks.get() + count);
        Ok(Vec::new())
    }

    fn get_chain_tip(&self) -> Result<u32, Error> {
        Ok(0)
    }

    fn get_filter_tip(&self) -> Result<u32, Error> {
        Ok(0)
    }

    fn get_block_hash(&self, _height: u32) -> Result<BlockHash, Error> {
        unreachable!("indexer preparation tests do not request block hashes")
    }

    fn call(&self, method: &str, _args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        assert_eq!(method, "getblockchaininfo");
        if self.fail_blockchain_info {
            return Err(Error::UnexpectedResponse(
                "blockchain information failed".to_string(),
            ));
        }
        Ok(self.blockchain_info.clone())
    }

    fn get_p2p_socket(&self) -> core::net::SocketAddr {
        core::net::SocketAddr::from(([127, 0, 0, 1], 18_444))
    }

    fn has_peer(&self, _socket: core::net::SocketAddr) -> Result<bool, Error> {
        Ok(false)
    }

    fn add_peer(&self, _socket: core::net::SocketAddr) -> Result<(), Error> {
        Ok(())
    }

    fn get_peer_count(&self) -> Result<u32, Error> {
        Ok(0)
    }
}

/// Create a temporary Unix program with the requested executable state.
#[cfg(unix)]
pub(super) fn test_program(body: &str, executable: bool) -> (TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test-program");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mode = if executable { 0o700 } else { 0o600 };
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    (directory, path)
}

/// Start a local Electrum server with a version handshake and scripted responses.
pub(super) fn scripted_electrum_socket(
    responses: Vec<Option<Result<serde_json::Value, serde_json::Value>>>,
) -> (core::net::SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let version_request: serde_json::Value = serde_json::from_str(&request).unwrap();
        let version_response = serde_json::json!({
            "id": version_request["id"].clone(),
            "result": ["halfin-test", "1.4"]
        });
        writeln!(stream, "{version_response}").unwrap();

        for response in responses {
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let Some(response) = response else {
                return;
            };
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            let id = request["id"].clone();
            let response = match response {
                Ok(result) => serde_json::json!({ "id": id, "result": result }),
                Err(error) => serde_json::json!({ "id": id, "error": error }),
            };
            writeln!(stream, "{response}").unwrap();
        }
    });
    (socket, handle)
}

/// Connect a raw Electrum client to a one-request scripted server.
pub(super) fn scripted_electrum_client(
    response: Option<Result<serde_json::Value, serde_json::Value>>,
) -> (RawClient<ElectrumPlaintextStream>, JoinHandle<()>) {
    let (socket, handle) = scripted_electrum_socket(vec![response]);
    let client = RawClient::new(socket, Some(Duration::from_secs(1)), None).unwrap();
    (client, handle)
}

/// Return the indexer error inside a common error.
fn indexer_error(error: Error) -> IndexerError {
    let Error::Indexer(error) = error else {
        panic!("expected an indexer error")
    };
    error
}

/// Verify RPC cookie parsing and validation.
#[test]
fn backing_node_cookie_is_validated() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }));

    assert!(matches!(read_backend_cookie(&node), Err(Error::Io(_))));

    for credentials in ["", "user", ":password", "user:"] {
        node.write_cookie(credentials);
        let error = read_backend_cookie(&node).unwrap_err();
        assert!(matches!(
            indexer_error(error),
            IndexerError::InvalidConfiguration(_)
        ));
    }

    node.write_cookie("user:password\n");
    let (path, credentials) = read_backend_cookie(&node).unwrap();
    assert_eq!(path, node.directory.path().join(RPC_COOKIE_FILE_NAME));
    assert_eq!(credentials, "user:password");
}

/// Verify indexer preparation generates only when a regtest backend needs a block.
#[test]
fn backing_node_readiness_generates_when_required() {
    let node = FakeNode::new(
        Network::Regtest,
        serde_json::json!({ "initialblockdownload": true, "blocks": 10 }),
    );
    ensure_backend_ready(&node, Network::Regtest, "TestIndexer").unwrap();
    assert_eq!(node.generated_blocks(), 1);

    let node = FakeNode::new(
        Network::Regtest,
        serde_json::json!({ "initialblockdownload": false, "blocks": 1 }),
    );
    ensure_backend_ready(&node, Network::Regtest, "TestIndexer").unwrap();
    assert_eq!(node.generated_blocks(), 0);

    let node = FakeNode::new(Network::Regtest, serde_json::json!({}));
    ensure_backend_ready(&node, Network::Regtest, "TestIndexer").unwrap();
    assert_eq!(node.generated_blocks(), 1);

    let node = FakeNode::new(
        Network::Bitcoin,
        serde_json::json!({ "initialblockdownload": true, "blocks": 0 }),
    );
    ensure_backend_ready(&node, Network::Bitcoin, "TestIndexer").unwrap();
    assert_eq!(node.generated_blocks(), 0);
}

/// Verify errors from readiness RPC and block generation are preserved.
#[test]
fn backing_node_readiness_propagates_backend_errors() {
    let node = FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 1 }))
        .with_blockchain_info_error();
    assert!(matches!(
        ensure_backend_ready(&node, Network::Regtest, "TestIndexer"),
        Err(Error::UnexpectedResponse(_))
    ));

    let node =
        FakeNode::new(Network::Regtest, serde_json::json!({ "blocks": 0 })).with_generation_error();
    assert!(matches!(
        ensure_backend_ready(&node, Network::Regtest, "TestIndexer"),
        Err(Error::UnexpectedResponse(_))
    ));
}

/// Maximum number of concurrent [`ElectrumxD`] tests.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
const ELECTRUMX_TEST_CONCURRENCY: usize = 2;

/// State that limits concurrent [`ElectrumxD`] tests.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
static ELECTRUMX_TEST_STATE: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// Permit to run one [`ElectrumxD`] test.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
#[derive(Debug)]
pub(super) struct ElectrumxTestPermit;

#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
impl Drop for ElectrumxTestPermit {
    fn drop(&mut self) {
        let (active, available) = &ELECTRUMX_TEST_STATE;
        let mut active = active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active -= 1;
        available.notify_one();
    }
}

/// Wait for permission to run an [`ElectrumxD`] test.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
pub(super) fn electrumx_test_permit() -> ElectrumxTestPermit {
    let (active, available) = &ELECTRUMX_TEST_STATE;
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    while *active >= ELECTRUMX_TEST_CONCURRENCY {
        active = available
            .wait(active)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }

    *active += 1;
    ElectrumxTestPermit
}

/// Consensus values returned by an [`Indexer`] for one script and transaction.
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
#[derive(Debug, PartialEq, Eq)]
struct IndexedValues {
    /// Hash of the block at the selected height.
    block_hash: BlockHash,

    /// Transaction history without optional server-specific fee metadata.
    history: Vec<(Txid, i32)>,

    /// Confirmed and unconfirmed script balances.
    balance: (u64, i64),

    /// Unspent outputs as transaction ID, height, output index, and value.
    unspent: Vec<(Txid, usize, usize, u64)>,

    /// Serialized transaction.
    transaction: Vec<u8>,
}

/// Read comparable consensus values from an [`Indexer`].
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
fn indexed_values(
    indexer: &impl Indexer,
    height: u32,
    script_pubkey: &Script,
    txid: Txid,
) -> IndexedValues {
    let client = Indexer::get_electrum_client(indexer);
    let height = usize::try_from(height).unwrap();

    let mut history = client
        .script_get_history(script_pubkey)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tx_hash, entry.height))
        .collect::<Vec<_>>();
    history.sort_unstable();

    let balance = client.script_get_balance(script_pubkey).unwrap();

    let mut unspent = client
        .script_list_unspent(script_pubkey)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tx_hash, entry.height, entry.tx_pos, entry.value))
        .collect::<Vec<_>>();
    unspent.sort_unstable();

    IndexedValues {
        block_hash: client.block_header(height).unwrap().block_hash(),
        history,
        balance: (balance.confirmed, balance.unconfirmed),
        unspent,
        transaction: client.transaction_get_raw(&txid).unwrap(),
    }
}

/// Poll [`ElectrumxD`] until it reports a transaction at its confirmation height.
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
pub(super) fn wait_until_electrumx_confirms_transaction(
    electrumxd: &ElectrumxD,
    script_pubkey: &Script,
    txid: Txid,
    confirmation_height: u32,
) {
    let confirmation_height = i32::try_from(confirmation_height).unwrap();
    let start = std::time::Instant::now();
    loop {
        let heights = electrumxd
            .client
            .script_get_history(script_pubkey)
            .unwrap()
            .into_iter()
            .filter_map(|entry| (entry.tx_hash == txid).then_some(entry.height))
            .collect::<Vec<_>>();
        if heights == [confirmation_height] {
            return;
        }
        assert!(
            start.elapsed() < crate::indexer::electrumxd::ELECTRUMX_INDEXING_TIMEOUT,
            "{} did not report transaction {txid} at height {confirmation_height}: heights={heights:?}",
            ElectrumxD::get_name()
        );
        std::thread::sleep(2 * crate::POLL_INTERVAL);
    }
}

/// Create a mempool transaction that pays a new address.
#[cfg(feature = "bitcoind")]
fn build_transaction(bitcoind: &BitcoinD) -> (ScriptBuf, Txid) {
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let txid = bitcoind
        .client
        .send_to_address(&address, Amount::from_int_btc(1))
        .unwrap()
        .txid()
        .unwrap();

    (script_pubkey, txid)
}

/// Verify the complete [`Indexer`] interface.
#[cfg(feature = "bitcoind")]
fn assert_indexer_interface<I>(
    indexer: &mut I,
    config: &I::Config,
    bitcoind: &BitcoinD,
    script_pubkey: &Script,
    txid: Txid,
) where
    I: Indexer,
    I::Config: Debug + PartialEq,
{
    assert!(!I::get_name().is_empty());
    assert!(!I::get_bin_name().is_empty());
    assert!(Indexer::get_pid(indexer) > 0);
    assert!(Indexer::get_working_directory(indexer).is_dir());
    assert_eq!(Indexer::get_config(indexer), config);

    let socket = Indexer::get_electrum_socket(indexer);
    assert!(socket.ip().is_loopback());
    assert_eq!(Indexer::get_electrum_url(indexer), socket.to_string());
    Indexer::get_electrum_client(indexer).ping().unwrap();

    Indexer::trigger(indexer).unwrap();
    Indexer::wait_until_caught_up(indexer, bitcoind, None).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();
    Indexer::wait_until_tip(indexer, height, block_hash, None).unwrap();
    Indexer::wait_until_mempool_tx(indexer, script_pubkey, txid, None).unwrap();
    Indexer::stop(indexer).unwrap();
}

/// Verify the [`Indexer`] interface for [`ElectrsD`].
#[cfg(all(feature = "bitcoind", feature = "electrs"))]
#[test]
fn electrsd_implements_indexer() {
    let bitcoind = BitcoinD::new().unwrap();
    let (script_pubkey, txid) = build_transaction(&bitcoind);
    let config = ElectrsDConf::default();
    let mut electrsd = ElectrsD::new_with_conf(&bitcoind, &config).unwrap();

    assert_indexer_interface(&mut electrsd, &config, &bitcoind, &script_pubkey, txid);
}

/// Verify the [`Indexer`] interface for [`ElectrumxD`].
#[cfg(all(feature = "bitcoind", feature = "electrumx"))]
#[test]
fn electrumxd_implements_indexer() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    let (script_pubkey, txid) = build_transaction(&bitcoind);
    let config = ElectrumxDConf::default();
    let mut electrumxd = ElectrumxD::new_with_conf(&bitcoind, &config).unwrap();

    assert_indexer_interface(&mut electrumxd, &config, &bitcoind, &script_pubkey, txid);
}

/// Verify that [`ElectrsD`] and [`ElectrumxD`] index the same values.
#[cfg(all(feature = "bitcoind", feature = "electrs", feature = "electrumx"))]
#[test]
fn electrsd_and_electrumxd_index_same_values() {
    let _permit = electrumx_test_permit();
    let bitcoind = BitcoinD::new().unwrap();
    bitcoind.generate(MATURE_COINBASE_BLOCK_COUNT).unwrap();

    let electrsd = ElectrsD::new(&bitcoind).unwrap();
    let electrumxd = ElectrumxD::new(&bitcoind).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let height = bitcoind.get_chain_tip().unwrap();
    let block_hash = bitcoind.get_block_hash(height).unwrap();

    let address = bitcoind.client.new_address().unwrap();
    let script_pubkey = address.script_pubkey();
    let amount = Amount::from_int_btc(1);
    let txid = bitcoind
        .client
        .send_to_address(&address, amount)
        .unwrap()
        .txid()
        .unwrap();

    electrsd
        .wait_until_mempool_tx(&script_pubkey, txid, None)
        .unwrap();
    electrumxd
        .wait_until_mempool_tx(&script_pubkey, txid, None)
        .unwrap();

    let electrs_mempool = indexed_values(&electrsd, height, &script_pubkey, txid);
    let electrumx_mempool = indexed_values(&electrumxd, height, &script_pubkey, txid);
    assert_eq!(electrs_mempool, electrumx_mempool);
    assert_eq!(electrs_mempool.block_hash, block_hash);
    assert_eq!(electrs_mempool.history, [(txid, 0)]);
    assert_eq!(
        electrs_mempool.balance,
        (0, i64::try_from(amount.to_sat()).unwrap())
    );
    assert_eq!(electrs_mempool.unspent.len(), 1);
    assert_eq!(electrs_mempool.unspent[0].0, txid);
    assert_eq!(electrs_mempool.unspent[0].1, 0);
    assert_eq!(electrs_mempool.unspent[0].3, amount.to_sat());

    bitcoind.generate(CONFIRMATION_BLOCK_COUNT).unwrap();
    electrsd.wait_until_caught_up(&bitcoind, None).unwrap();
    electrumxd.wait_until_caught_up(&bitcoind, None).unwrap();

    let confirmation_height = height + 1;
    wait_until_electrumx_confirms_transaction(
        &electrumxd,
        &script_pubkey,
        txid,
        confirmation_height,
    );
    let height = bitcoind.get_chain_tip().unwrap();
    let electrs_confirmed = indexed_values(&electrsd, height, &script_pubkey, txid);
    let electrumx_confirmed = indexed_values(&electrumxd, height, &script_pubkey, txid);
    assert_eq!(electrs_confirmed, electrumx_confirmed);
    assert_eq!(
        electrs_confirmed.block_hash,
        bitcoind.get_block_hash(height).unwrap()
    );
    assert_eq!(
        electrs_confirmed.history,
        [(txid, i32::try_from(confirmation_height).unwrap())]
    );
    assert_eq!(electrs_confirmed.balance, (amount.to_sat(), 0));
    assert_eq!(electrs_confirmed.unspent.len(), 1);
    assert_eq!(electrs_confirmed.unspent[0].0, txid);
    assert_eq!(
        electrs_confirmed.unspent[0].1,
        usize::try_from(confirmation_height).unwrap()
    );
    assert_eq!(electrs_confirmed.unspent[0].3, amount.to_sat());
    assert_eq!(electrs_confirmed.transaction, electrs_mempool.transaction);

    let confirmation_height = usize::try_from(confirmation_height).unwrap();
    let electrs_merkle = electrsd
        .client
        .transaction_get_merkle(&txid, confirmation_height)
        .unwrap();
    let electrumx_merkle = electrumxd
        .client
        .transaction_get_merkle(&txid, confirmation_height)
        .unwrap();
    assert_eq!(electrs_merkle.block_height, confirmation_height);
    assert_eq!(electrumx_merkle.block_height, confirmation_height);
    assert_eq!(electrs_merkle.pos, electrumx_merkle.pos);
    assert_eq!(electrs_merkle.merkle, electrumx_merkle.merkle);
}
