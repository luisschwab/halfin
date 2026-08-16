// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared integration tests for [`Node`] implementations.
//!
//! These tests apply the [`Node`] interface to each enabled implementation.
//!
//! [`Node`]: crate::node::Node

#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use core::net::SocketAddr;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::fs;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::thread::sleep;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use std::time::Instant;

use super::Node;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use super::RPC_COOKIE_FILE_NAME;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use super::RPC_PASS;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use super::RPC_USER;
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
use super::connect;
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
use crate::CONNECTION_INTERVAL;
#[cfg(feature = "bitcoind")]
use crate::node::bitcoind::BitcoinD;
#[cfg(feature = "florestad")]
use crate::node::florestad::FlorestaD;
#[cfg(feature = "utreexod")]
use crate::node::utreexod::UtreexoD;

/// Wait until a [`Node`] connects to all specified peers.
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
pub(super) fn wait_for_fixed_peers<N: Node>(
    node: &N,
    peers: &[SocketAddr],
    timeout: core::time::Duration,
) {
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
#[cfg(any(feature = "bitcoind", feature = "utreexod"))]
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
