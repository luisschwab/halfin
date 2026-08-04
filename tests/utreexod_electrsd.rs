// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests between [`UtreexoD`] and [`ElectrsD`].

#![cfg(all(feature = "utreexod", feature = "electrs"))]

use halfin::Error;
use halfin::electrsd::ElectrsD;
use halfin::electrsd::ElectrsDConf;
use halfin::utreexod::UtreexoD;

/// Verify that `UtreexoD` is rejected before creating the indexer's data directory.
#[test]
fn test_electrsd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let indexer_dir = root.path().join("electrs");
    let electrsd_conf = ElectrsDConf {
        staticdir: Some(indexer_dir.clone()),
        ..ElectrsDConf::default()
    };

    match ElectrsD::new_with_conf(&utreexod, &electrsd_conf) {
        Err(Error::InvalidIndexerConfiguration(message)) => assert_eq!(
            message,
            "UtreexoD cannot currently be used as an indexer backing node"
        ),
        result => panic!("expected UtreexoD backend rejection, got {result:?}"),
    }
    assert!(!indexer_dir.exists());
}
