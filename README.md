<p align="center">
    <img src="static/halfin.webp" width="40%" alt="A Bitcoin Node Runner (Hal Finney)">
</p>

# halfin

<p>
    <a href="https://crates.io/crates/halfin"><img src="https://img.shields.io/crates/v/halfin.svg"/></a>
    <a href="https://docs.rs/halfin"><img src="https://img.shields.io/badge/docs.rs-halfin-green"/></a>
    <a href="https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/"><img src="https://img.shields.io/badge/rustc-1.85.0%2B-orange.svg?label=MSRV"/></a>
    <a href="https://github.com/luisschwab/halfin/blob/master/LICENSE"><img src="https://img.shields.io/badge/License-MIT%2FApache--2.0-red.svg"/></a>
    <a href="https://github.com/luisschwab/halfin/actions/workflows/rust.yml"><img src="https://github.com/luisschwab/halfin/actions/workflows/rust.yml/badge.svg"></a>
    <a href="https://github.com/luisschwab/halfin/actions/workflows/cross.yml"><img src="https://github.com/luisschwab/halfin/actions/workflows/cross.yml/badge.svg"></a>
</p>

> A runner for bitcoin nodes and indexers 🏃‍♂️

This crate makes it simple to run [`bitcoind`](https://github.com/bitcoin/bitcoin),
[`utreexod`](https://github.com/utreexo/utreexod), [`electrs`](https://github.com/romanz/electrs),
[`electrumx`](https://github.com/spesmilo/electrumx), and [`florestad`](https://github.com/getfloresta/Floresta)
instances from Rust code, useful in integration test contexts.

## Supported Implementations

| Kind    | Implementation | Version   | Feature Flag | Notes             |
|---------|----------------|-----------|--------------|-------------------|
| Node    | `bitcoind`     | `v31.0`   | `bitcoind`   |                   |
| Node    | `florestad`    | `v0.9.1`  | `florestad`  |                   |
| Node    | `utreexod`     | `v0.6.0`  | `utreexod`   |                   |
|         |                |           |              |                   |
| Indexer | `electrs`      | `v0.11.1` | `electrs`    |                   |
| Indexer | `electrumx`    | `v1.20.0` | `electrumx`  | Needs Python 3.10 |

Binaries are downloaded automatically at build time: see [`build.rs`](./build.rs).

### BitcoinD

```rs
use std::path::PathBuf;

use halfin::node::bitcoind::BitcoinD;
use halfin::node::{connect, wait_for_height};

// Use a downloaded binary
let bitcoind_alpha = BitcoinD::new().unwrap();

// Use a local binary
let bin_path = PathBuf::from("/usr/local/bin/bitcoind");
let bitcoind_beta = BitcoinD::from_bin(&bin_path).unwrap();

// Connect peers
connect(&bitcoind_alpha, &bitcoind_beta).unwrap();

// Mine blocks
bitcoind_alpha.generate(100).unwrap();
assert_eq!(bitcoind_alpha.get_chain_tip().unwrap(), 100);

// Wait for a node to catch up with the other
wait_for_height(&bitcoind_beta, 100).unwrap();
assert_eq!(bitcoind_beta.get_chain_tip().unwrap(), 100);
```

### ElectrsD

```rust
use halfin::indexer::electrsd::ElectrsD;
use halfin::node::bitcoind::BitcoinD;

let bitcoind = BitcoinD::new().unwrap();
bitcoind.generate(100).unwrap();

let electrs = ElectrsD::new(&bitcoind).unwrap();
electrs.wait_until_caught_up(&bitcoind, None).unwrap();
```

### UtreexoD

```rust
use halfin::node::utreexod::UtreexoD;

// Use a downloaded binary
let utreexod = UtreexoD::new().unwrap();

// Mine blocks
utreexod.generate(100).unwrap();
assert_eq!(utreexod.get_chain_tip().unwrap(), 100);

// Perform a raw RPC call
let res = utreexod.call("uptime", &[]).unwrap();
```

### FlorestaD

```rust
TODO
```

## Developing

This project uses [`just`](https://github.com/casey/just) for command running, and
[`cargo-rbmt`](https://github.com/rust-bitcoin/rust-bitcoin-maintainer-tools/tree/master/cargo-rbmt)
to manage everything related to `cargo`, such as formatting, linting, testing and CI. To install them, run:

```shell
~$ cargo install just

~$ cargo install cargo-rbmt
```

A `justfile` is provided for convenience. Run `just` to see available commands:

```shell
> halfin
> A regtest runner for `bitcoind` and `utreexod`

Available recipes:
    audit      # Run cargo-audit across all lockfiles and prune stale advisories [alias: a]
    build      # Build `halfin` [alias: b]
    check      # Check Formatting, Linting and Documentation [alias: c]
    doc        # Generate Documentation [alias: d]
    doc-open   # Generate and Open Documentation [alias: do]
    fmt        # Format Code [alias: f]
    lock       # Regenerate Lockfiles [alias: l]
    pre-push   # Run pre-push checks [alias: p]
    shellcheck # Run ShellCheck [alias: sc]
    test       # Run Tests [alias: t]
    test-all   # Run Tests with Lockfile and Toolchain Combos
    toolchains # Update Stable and Nightly Toolchains
    tools      # Install cargo-rbmt Tools
    zizmor     # Run Zizmor Static Analysis [alias: z]
```

## Minimum Supported Rust Version

This library should compile with any combination of features on Rust 1.85.0.

To build with the MSRV toolchain, copy `Cargo-minimal.lock` to `Cargo.lock`.

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
