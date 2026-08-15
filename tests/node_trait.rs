// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the [`Node`] trait.
//!
//! These tests apply the same RPC and connection operations to each enabled [`Node`].
//!
//! [`Node`]: halfin::node::Node

#![cfg(any(feature = "bitcoind", feature = "utreexod"))]

use halfin::node::Node;
#[cfg(feature = "bitcoind")]
use halfin::node::bitcoind::BitcoinD;
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
use halfin::node::connect;
#[cfg(feature = "utreexod")]
use halfin::node::utreexod::UtreexoD;

/// Verify the location, contents, and Unix permissions of the shared RPC cookie.
fn assert_rpc_cookie<N: Node>(node: &N) {
    let cookie_file = node.get_working_directory().join(".cookie");

    assert_eq!(
        std::fs::read_to_string(&cookie_file).unwrap(),
        "__cookie__:halfin"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(cookie_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

/// Verify that every enabled [`Node`] supplies the shared RPC cookie.
#[test]
fn test_rpc_cookies() {
    #[cfg(feature = "bitcoind")]
    assert_rpc_cookie(&BitcoinD::new().unwrap());

    #[cfg(feature = "utreexod")]
    assert_rpc_cookie(&UtreexoD::new().unwrap());
}

/// Verify [`Node::call`] with the `uptime` method.
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
#[test]
fn test_node_call() {
    let bitcoind = BitcoinD::new().unwrap();
    let utreexod = UtreexoD::new().unwrap();

    let bitcoind_uptime = bitcoind.call("uptime", &[]).unwrap();
    let utreexod_uptime = utreexod.call("uptime", &[]).unwrap();

    println!("BitcoinD uptime: {}", bitcoind_uptime);
    println!("UtreexoD uptime: {}", utreexod_uptime);
}

/// Verify [`connect`] with each combination of [`Node`] implementations.
#[cfg(all(feature = "bitcoind", feature = "utreexod"))]
#[test]
fn test_connect() {
    let bitcoind_1 = BitcoinD::new().unwrap();
    let bitcoind_2 = BitcoinD::new().unwrap();
    let utreexod_1 = UtreexoD::new().unwrap();
    let utreexod_2 = UtreexoD::new().unwrap();

    // .. > bitcoind_1 > bitcoind_2 > utreexod_1 > utreexod_2 > ..
    connect(&bitcoind_1, &bitcoind_2).unwrap();
    connect(&bitcoind_2, &utreexod_1).unwrap();
    connect(&utreexod_1, &utreexod_2).unwrap();
    connect(&utreexod_2, &bitcoind_1).unwrap();
}
