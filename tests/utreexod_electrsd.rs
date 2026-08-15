// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration test for an unsupported [`UtreexoD`] and [`ElectrsD`] combination.
//!
//! This test verifies that validation occurs before data directory creation.

#![cfg(all(feature = "utreexod", feature = "electrs"))]

use halfin::Error;
use halfin::indexer::IndexerError;
use halfin::indexer::electrsd::ElectrsD;
use halfin::indexer::electrsd::ElectrsDConf;
use halfin::node::utreexod::UtreexoD;

/// Verify that rejection of `UtreexoD` occurs before [`Indexer`](halfin::indexer::Indexer)
/// directory creation.
#[test]
fn test_electrsd_rejects_utreexod() {
    let utreexod = UtreexoD::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let indexer_dir = root.path().join("electrs");
    let electrsd_conf = ElectrsDConf {
        staticdir: Some(indexer_dir.clone()),
        ..ElectrsDConf::default()
    };

    assert!(matches!(
        ElectrsD::new_with_conf(&utreexod, &electrsd_conf),
        Err(Error::Indexer(IndexerError::UnsupportedBackend {
            node: "UtreexoD"
        }))
    ));
    assert!(!indexer_dir.exists());
}
