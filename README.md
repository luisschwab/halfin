<p align="center">
    <img src="asset/image/halfin.webp" width="40%" alt="A Bitcoin Node Runner (Hal Finney)">
</p>

# halfin

[![crates.io][crates-badge]](https://crates.io/crates/halfin)
[![docs.rs][docs-badge]](https://docs.rs/halfin)
[![rustc][rustc-badge]](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
[![license-mit-apache][license-badge]](https://github.com/luisschwab/halfin/blob/master/LICENSE-MIT)
[![test suite][rust-badge]](https://github.com/luisschwab/halfin/actions/workflows/rust.yml)
[![cross builds][cross-badge]](https://github.com/luisschwab/halfin/actions/workflows/cross.yml)
[![Coverage][coverage-badge]](https://codecov.io/gh/luisschwab/halfin)

[crates-badge]: https://img.shields.io/crates/v/halfin.svg
[docs-badge]: https://img.shields.io/badge/docs.rs-halfin-green
[rustc-badge]: https://img.shields.io/badge/rustc-1.85.0%2B-orange.svg?label=MSRV
[license-badge]: https://img.shields.io/badge/License-MIT%2FApache--2.0-red.svg
[rust-badge]: https://github.com/luisschwab/halfin/actions/workflows/rust.yml/badge.svg
[cross-badge]: https://github.com/luisschwab/halfin/actions/workflows/cross.yml/badge.svg
[coverage-badge]: https://img.shields.io/codecov/c/github/luisschwab/halfin/master?label=Coverage

> A runner for bitcoin nodes and indexers 🏃‍♂️

Use this crate to start Bitcoin nodes and indexers from Rust tests. It starts the
implementations in the table below and gives each process its own data directory.

## Supported Implementations

| Kind         | Implementation          | Version   | Feature Flag          | Notes                                 |
|--------------|-------------------------|-----------|-----------------------|---------------------------------------|
| Node         | [`Bitcoin Core`]        | `v31.0`   | `bitcoind`            |                                       |
| Node         | [`btcd`]                | `v0.26.2` | `btcd`                |                                       |
| Node         | [`Floresta`]            | `v0.9.1`  | `florestad`           |                                       |
| Node         | [`utreexod`]            | `v0.6.0`  | `utreexod`            |                                       |
|              |                         |           |                       |                                       |
| Indexer      | [`Blockstream/electrs`] | `4b1a018` | `blockstream_electrs` | Unsupported on Windows                |
| Indexer      | [`ElectrumX`]           | `v1.20.0` | `electrumx`           | Needs Python 3.10                     |
| Indexer      | [`Frigate`]             | `v1.5.3`  | `frigate`             | Unsupported on Windows                |
| Indexer      | [`mempool/electrs`]     | `v3.3.0`  | `mempool_electrs`     | Unsupported on Windows                |
| Indexer      | [`romanz/electrs`]      | `v0.12.0` | `romanz_electrs`      |                                       |
|              |                         |           |                       |                                       |
| Node/Indexer | [`libbitcoin-server`]   | `3620f1d` | `libbitcoin`          | Unsupported on Windows / Mainnet-only |

[`Bitcoin Core`]: <https://github.com/bitcoin/bitcoin>
[`bitcoind`]: <https://github.com/bitcoin/bitcoin>
[`btcd`]: <https://github.com/btcsuite/btcd>
[`Floresta`]: <https://github.com/getfloresta/Floresta>
[`florestad`]: <https://github.com/getfloresta/Floresta>
[`utreexod`]: <https://github.com/utreexo/utreexod>
[`Blockstream/electrs`]: <https://github.com/Blockstream/electrs>
[`ElectrumX`]: <https://github.com/spesmilo/electrumx>
[`Frigate`]: <https://github.com/sparrowwallet/frigate>
[`mempool/electrs`]: <https://github.com/mempool/electrs>
[`romanz/electrs`]: <https://github.com/romanz/electrs>
[`libbitcoin-server`]: <https://github.com/libbitcoin/libbitcoin-server>

When you enable a feature, [`build.rs`](./build.rs) downloads the required
executable during compilation. It checks the archive against a SHA-256 checksum.

### BitcoinD

```rs
use std::path::PathBuf;

use halfin::node::bitcoind::BitcoinD;
use halfin::node::{connect, wait_for_height};

// Start the downloaded executable.
let bitcoind_alpha = BitcoinD::new().unwrap();

// Start a local executable.
let bin_path = PathBuf::from("/usr/local/bin/bitcoind");
let bitcoind_beta = BitcoinD::from_bin(&bin_path).unwrap();

// Connect the two nodes.
connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

// Mine 100 blocks.
bitcoind_alpha.generate(100).unwrap();
assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 100);

// Wait until the second node reaches block 100.
wait_for_height(&bitcoind_beta, 100).unwrap();
assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 100);
```

### BtcD

```rust
use halfin::node::btcd::BtcD;

let btcd = BtcD::new().unwrap();

btcd.generate(100).unwrap();
assert_eq!(btcd.get_chain_tip().unwrap(), 100);
```

### RomanzElectrsD

```rust
use halfin::indexer::romanz_electrsd::RomanzElectrsD;
use halfin::node::bitcoind::BitcoinD;

let bitcoind = BitcoinD::new().unwrap();
bitcoind.generate(100).unwrap();

let electrs = RomanzElectrsD::new(&bitcoind).unwrap();
electrs.wait_until_caught_up(&bitcoind, None).unwrap();
```

### BlockstreamElectrsD

```rust
use halfin::indexer::blockstream_electrsd::BlockstreamElectrsD;
use halfin::node::bitcoind::BitcoinD;

let bitcoind = BitcoinD::new().unwrap();
bitcoind.generate(100).unwrap();
let indexer = BlockstreamElectrsD::new(&bitcoind).unwrap();
indexer.wait_until_caught_up(&bitcoind, None).unwrap();
assert_eq!(indexer.get_esplora_client().get_height().unwrap(), 100);
```

### FrigateD

```rust
use halfin::indexer::frigated::FrigateD;
use halfin::indexer::romanz_electrsd::RomanzElectrsD;
use halfin::node::bitcoind::BitcoinD;

let bitcoind = BitcoinD::new().unwrap();
bitcoind.generate(100).unwrap();
let backend = RomanzElectrsD::new(&bitcoind).unwrap();
backend.wait_until_caught_up(&bitcoind, None).unwrap();

let frigate = FrigateD::new(&bitcoind, &backend).unwrap();
frigate.wait_until_caught_up(&bitcoind, None).unwrap();
```

### UtreexoD

```rust
use halfin::node::utreexod::UtreexoD;

// Start the downloaded executable.
let utreexod = UtreexoD::new().unwrap();

// Mine 100 blocks.
utreexod.generate(100).unwrap();
assert_eq!(utreexod.get_chain_tip().unwrap(), 100);

// Call an RPC method.
let res = utreexod.call("uptime", &[]).unwrap();
```

### FlorestaD

```rust
use halfin::node::florestad::FlorestaD;
use halfin::node::utreexod::UtreexoD;
use halfin::node::{connect_and_sync, wait_for_height};

// Mine 10 blocks with a Utreexo peer.
let utreexod = UtreexoD::new().unwrap();
utreexod.generate(10).unwrap();

// Wait until Utreexo reaches block 10.
wait_for_height(&utreexod, 10).unwrap();

// Connect Floresta to Utreexo. Wait until Floresta reaches block 10.
let florestad = FlorestaD::new().unwrap();
connect_and_sync(&florestad, &utreexod).unwrap();

assert_eq!(florestad.get_chain_tip().unwrap(), 10);
```

## Developing

The project uses [`just`] to run commands. It uses [`cargo-rbmt`] for formatting,
linting, tests, and documentation. Install both tools:

[`just`]: <https://github.com/casey/just>
[`cargo-rbmt`]: <https://github.com/rust-bitcoin/rust-bitcoin-maintainer-tools/tree/master/cargo-rbmt>

```shell
~$ cargo install just

~$ cargo install cargo-rbmt
```

Run this command to list the available recipes:

```shell
just
```

## Minimum Supported Rust Version

Rust 1.85.0 is the minimum supported version. The crate supports all feature
combinations on this version.

To build with the minimum supported Rust version, copy `Cargo-minimal.lock` to
`Cargo.lock`.

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
