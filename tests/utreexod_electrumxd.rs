// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`UtreexoD`] and [`ElectrumxD`].

#![cfg(all(feature = "utreexod", feature = "electrumx"))]

use halfin::Error;
use halfin::electrumxd::ElectrumxD;
use halfin::electrumxd::ElectrumxDConf;
use halfin::utreexod::UtreexoD;

/// Verify that `UtreexoD` is rejected before creating the indexer's data directory.
#[test]
fn test_electrumxd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let indexer_dir = root.path().join("electrumx");
    let electrumxd_conf = ElectrumxDConf {
        staticdir: Some(indexer_dir.clone()),
        ..ElectrumxDConf::default()
    };

    match ElectrumxD::new_with_conf(&utreexod, &electrumxd_conf) {
        Err(Error::InvalidIndexerConfiguration(message)) => assert_eq!(
            message,
            "UtreexoD cannot currently be used as an indexer backing node"
        ),
        result => panic!("expected UtreexoD backend rejection, got {result:?}"),
    }
    assert!(!indexer_dir.exists());
}
