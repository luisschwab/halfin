// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared integration tests for [`Node`] implementations.
//!
//! These tests apply the [`Node`] interface to each enabled implementation.
//!
//! [`Node`]: crate::node::Node

use core::net::SocketAddr;
use core::time::Duration;
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use std::fs;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use std::io::Read;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use std::io::Write;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use std::net::TcpListener;
use std::path::PathBuf;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use std::thread::JoinHandle;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use std::thread::sleep;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
use tempfile::TempDir;

use super::Node;
use super::NodeArgs;
use super::PruneMode;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use super::RPC_COOKIE_FILE_NAME;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use super::RPC_PASS;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use super::RPC_USER;
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
use super::connect;
use super::connect_with_timeout;
use super::wait_for_filter_height;
use super::wait_for_height;
use super::wait_for_height_with_timeout;
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
use crate::CONNECTION_INTERVAL;
use crate::Error;
use crate::node::NodeError;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "btcd")]
use crate::node::btcd::BtcD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Configuration for [`FakeNode`].
#[derive(Debug)]
struct FakeNodeConfig(NodeArgs);

impl Default for FakeNodeConfig {
    fn default() -> Self {
        Self(NodeArgs {
            network: Network::Regtest,
            fixed_peers: Vec::new(),
            v2_transport: false,
            cbf_index: false,
            prune: PruneMode::Disabled,
            txindex: false,
        })
    }
}

impl AsRef<NodeArgs> for FakeNodeConfig {
    fn as_ref(&self) -> &NodeArgs {
        &self.0
    }
}

/// Deterministic node used to test shared wait behavior.
#[derive(Debug, Default)]
struct FakeNode {
    config: FakeNodeConfig,
    chain_tip: Cell<u32>,
    filter_tip: Cell<u32>,
    peer_results: RefCell<VecDeque<bool>>,
    add_peer_calls: Cell<u32>,
    fail_add_peer: bool,
    fail_peer_query: bool,
}

impl FakeNode {
    /// Create a node with a sequence of peer-query results.
    fn with_peer_results(results: impl IntoIterator<Item = bool>) -> Self {
        Self {
            peer_results: RefCell::new(results.into_iter().collect()),
            ..Self::default()
        }
    }
}

impl Node for FakeNode {
    type Config = FakeNodeConfig;

    fn get_name() -> &'static str {
        "FakeNode"
    }

    fn get_bin_name() -> &'static str {
        "test-node"
    }

    fn get_config(&self) -> &Self::Config {
        &self.config
    }

    fn get_working_directory(&self) -> PathBuf {
        PathBuf::new()
    }

    fn get_rpc_socket(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 18_443))
    }

    fn generate(&self, _count: u32) -> Result<Vec<BlockHash>, Error> {
        Ok(Vec::new())
    }

    fn get_chain_tip(&self) -> Result<u32, Error> {
        Ok(self.chain_tip.get())
    }

    fn get_filter_tip(&self) -> Result<u32, Error> {
        Ok(self.filter_tip.get())
    }

    fn get_block_hash(&self, _height: u32) -> Result<BlockHash, Error> {
        unreachable!("shared wait tests do not request block hashes")
    }

    fn call(&self, _method: &str, _args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        Ok(serde_json::Value::Null)
    }

    fn get_p2p_socket(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 18_444))
    }

    fn has_peer(&self, _socket: SocketAddr) -> Result<bool, Error> {
        if self.fail_peer_query {
            return Err(Error::UnexpectedResponse("peer query failed".to_string()));
        }
        Ok(self.peer_results.borrow_mut().pop_front().unwrap_or(false))
    }

    fn add_peer(&self, _socket: SocketAddr) -> Result<(), Error> {
        self.add_peer_calls.set(self.add_peer_calls.get() + 1);
        if self.fail_add_peer {
            return Err(Error::UnexpectedResponse(
                "peer addition failed".to_string(),
            ));
        }
        Ok(())
    }

    fn get_peer_count(&self) -> Result<u32, Error> {
        Ok(0)
    }

    fn poll_interval() -> Duration {
        Duration::ZERO
    }

    fn wait_timeout() -> Duration {
        Duration::from_millis(1)
    }
}

/// Create a temporary Unix program with the requested executable state.
#[cfg(all(
    unix,
    any(
        feature = "bitcoind",
        feature = "btcd",
        feature = "florestad",
        feature = "utreexod"
    )
))]
pub(super) fn test_program(body: &str, executable: bool) -> (TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test-program");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mode = if executable { 0o700 } else { 0o600 };
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    (directory, path)
}

/// Serve a sequence of JSON-RPC results over one HTTP request per result.
#[cfg(any(
    feature = "bitcoind",
    feature = "btcd",
    feature = "florestad",
    feature = "utreexod"
))]
pub(super) fn scripted_json_rpc_server(
    results: Vec<serde_json::Value>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for result in results {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let header_end = loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "JSON-RPC client closed before sending headers");
                request.extend_from_slice(&buffer[..count]);
                if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            while request.len() - header_end < content_length {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "JSON-RPC client closed before sending its body");
                request.extend_from_slice(&buffer[..count]);
            }
            let request: serde_json::Value =
                serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap();
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"].clone(),
                "result": result,
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
    });
    (socket, handle)
}

/// Return the node error inside a common error.
fn node_error(error: Error) -> NodeError {
    let Error::Node(error) = error else {
        panic!("expected a node error")
    };
    error
}

/// Verify a transient peer result is checked again before connection succeeds.
#[test]
fn connection_rechecks_transient_peer_results() {
    let node_a = FakeNode::with_peer_results([true, false, true, true]);
    let node_b = FakeNode::default();

    connect_with_timeout(&node_a, &node_b, Duration::from_millis(5), Duration::ZERO).unwrap();
    assert_eq!(node_a.add_peer_calls.get(), 1);
}

/// Verify connection waits return their configured timeout.
#[test]
fn connection_reports_timeout() {
    let node_a = FakeNode::default();
    let node_b = FakeNode::default();
    let timeout = Duration::from_millis(1);

    let error = connect_with_timeout(&node_a, &node_b, timeout, Duration::ZERO).unwrap_err();
    assert!(matches!(
        node_error(error),
        NodeError::ConnectionTimeout(value) if value == timeout
    ));
}

/// Verify connection setup propagates peer-operation failures.
#[test]
fn connection_propagates_peer_errors() {
    let node_b = FakeNode::default();
    let node_a = FakeNode {
        fail_add_peer: true,
        ..FakeNode::default()
    };
    assert!(matches!(
        connect_with_timeout(&node_a, &node_b, Duration::from_millis(1), Duration::ZERO),
        Err(Error::UnexpectedResponse(_))
    ));

    let node_a = FakeNode {
        fail_peer_query: true,
        ..FakeNode::default()
    };
    assert!(matches!(
        connect_with_timeout(&node_a, &node_b, Duration::from_millis(1), Duration::ZERO),
        Err(Error::UnexpectedResponse(_))
    ));
}

/// Verify chain and filter waits recognize an available height.
#[test]
fn height_waits_accept_reached_heights() {
    let node = FakeNode::default();
    node.chain_tip.set(10);
    node.filter_tip.set(9);

    wait_for_height(&node, 10).unwrap();
    wait_for_height_with_timeout(&node, 10, Duration::from_millis(1)).unwrap();
    wait_for_filter_height(&node, 9).unwrap();
}

/// Verify chain and filter waits retain timeout context.
#[test]
fn height_waits_report_timeouts() {
    let node = FakeNode::default();
    node.chain_tip.set(9);
    node.filter_tip.set(8);

    let error = wait_for_height(&node, 10).unwrap_err();
    assert!(matches!(
        node_error(error),
        NodeError::ChainSyncTimeout((10, 9, timeout)) if timeout == FakeNode::wait_timeout()
    ));

    let timeout = Duration::from_millis(25);
    let error = wait_for_height_with_timeout(&node, 10, timeout).unwrap_err();
    assert!(matches!(
        node_error(error),
        NodeError::ChainSyncTimeout((10, 9, value)) if value == timeout
    ));

    let error = wait_for_filter_height(&node, 9).unwrap_err();
    assert!(matches!(
        node_error(error),
        NodeError::ChainSyncTimeout((9, 8, timeout)) if timeout == FakeNode::wait_timeout()
    ));
}

/// Wait until a [`Node`] connects to all specified peers.
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
pub(super) fn wait_for_fixed_peers<N: Node>(node: &N, peers: &[SocketAddr], timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if peers
            .iter()
            .all(|peer| node.has_peer(*peer).unwrap_or(false))
        {
            return;
        }
        sleep(CONNECTION_INTERVAL);
    }

    let peer_info = node.call("getpeerinfo", &[]).unwrap();
    for peer in peers {
        assert!(
            node.has_peer(*peer).unwrap(),
            "{} did not connect to fixed peer {peer}; peer info: {peer_info}",
            N::get_name(),
        );
    }
}

/// Verify the operations that all [`Node`] implementations support.
fn assert_node_interface<N: Node>(node: &N, name: &str) {
    assert_eq!(N::get_name(), name);
    assert!(!N::get_bin_name().is_empty());
    assert!(Node::get_working_directory(node).is_dir());
    assert!(Node::get_rpc_socket(node).ip().is_loopback());
    assert_eq!(Node::get_peer_count(node).unwrap(), 0);

    let uptime = Node::call(node, "uptime", &[]).unwrap();
    assert!(uptime.as_u64().is_some());
}

/// Verify the contents and Unix permissions of an RPC cookie.
#[cfg(any(feature = "bitcoind", feature = "btcd", feature = "utreexod"))]
fn assert_rpc_cookie<N: Node>(node: &N) {
    let cookie_file = node.get_working_directory().join(RPC_COOKIE_FILE_NAME);

    assert_eq!(
        fs::read_to_string(&cookie_file).unwrap(),
        format!("{RPC_USER}:{RPC_PASS}")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(cookie_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

/// Verify the [`Node`] interface and RPC cookie for [`BitcoinD`].
#[cfg(feature = "bitcoind")]
#[test]
fn bitcoind_implements_node() {
    let bitcoind = BitcoinD::new().unwrap();

    assert_node_interface(&bitcoind, "BitcoinD");
    assert_rpc_cookie(&bitcoind);
}

/// Verify the [`Node`] interface and RPC cookie for [`BtcD`].
#[cfg(feature = "btcd")]
#[test]
fn btcd_implements_node() {
    let btcd = BtcD::new().unwrap();

    assert_node_interface(&btcd, "BtcD");
    assert_rpc_cookie(&btcd);
}

/// Verify the [`Node`] interface for [`FlorestaD`].
#[cfg(feature = "florestad")]
#[test]
fn florestad_implements_node() {
    let florestad = FlorestaD::new().unwrap();

    assert_node_interface(&florestad, "FlorestaD");
}

/// Verify the [`Node`] interface and RPC cookie for [`UtreexoD`].
#[cfg(feature = "utreexod")]
#[test]
fn utreexod_implements_node() {
    let utreexod = UtreexoD::new().unwrap();

    assert_node_interface(&utreexod, "UtreexoD");
    assert_rpc_cookie(&utreexod);
}

/// Verify [`connect`] with both connection directions for [`BitcoinD`] and [`UtreexoD`].
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
#[test]
fn bitcoind_and_utreexod_connect() {
    let bitcoind_alpha = BitcoinD::new().unwrap();
    let bitcoind_beta = BitcoinD::new().unwrap();
    let utreexod_alpha = UtreexoD::new().unwrap();
    let utreexod_beta = UtreexoD::new().unwrap();

    connect(&bitcoind_alpha, &bitcoind_beta).unwrap();
    connect(&bitcoind_beta, &utreexod_alpha).unwrap();
    connect(&utreexod_alpha, &utreexod_beta).unwrap();
    connect(&utreexod_beta, &bitcoind_alpha).unwrap();
}
