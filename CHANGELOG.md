# Changelog

## [Unreleased]

* `ElectrumxD`: assert Python is available at runtime
* Fix naming inconsistencies
* Bump `bitcoin_hashes` to v1.1.0
* Add and apply `rustfmt.toml`
* Bump `cargo-rbmt` to v0.5.2
* Drop `sample_strategy = "all"` and manually select feature combinations
* Add the `Indexer` trait and let `ElectrsD` and `ElectrumxD` accept `&impl Node` backends
* Add typed, owned node and indexer configurations exposed through `get_config`
* Kill orphaned `electrumx.exe` processes on Windows

## [0.5.0] – 2026-07-01

* ElectrumX support
* Enable `txindex` on `BitcoinD` by default
* Use `ELECTRS_VERSION` from `electrsd/versions.rs` on the binary building script
* Move errors to `error.rs`
* Add and apply `rust-bitcoin` lints
* Condense RBMT metadata
* Add CI job to assert PR bisectability
* Use `[package.metadata.rbmt.tools]` for `cargo-audit` and `zizmor`
* Increase dependabot's cooldown to 60 days
* Bump `cargo-rbmt` to v0.4.1
* Bump `cargo-rbmt` to v0.4.0
* Bump `utreexod` to v0.6.0
* Add [`bin.lab.vinteum.org`](https://bin.lab.vinteum.org) binary download mirror
* Refactor `build.rs`
* Drop version suffix from feature names
* Lock modules behind the respective feature
* Lock `electrum-client` dependency behind `electrs_0_11_1` feature
* Add `BitcoinD::invalidate_blocks`
* Replace `ElectrsD` free-function wait helpers with `ElectrsD::wait_until_*`
  methods that have an optional `timeout` parameter.
* Make `ElectrsD` block waits verify both expected height and block hash using
  Electrum header notifications.
* Add `ElectrsD` reorg coverage to verify the indexer follows replacement chain tips.

[Unreleased]: https://github.com/luisschwab/halfin/compare/v0.5.0...HEAD

[0.5.0]: https://github.com/luisschwab/halfin/compare/v0.4.0...v0.5.0
