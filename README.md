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
and [`electrumx`](https://github.com/spesmilo/electrumx) instances from Rust code, useful in
integration test contexts.

## Supported Implementations

| Kind    | Implementation | Version   | Feature Flag | Default Feature | Notes             |
|---------|----------------|-----------|--------------|-----------------|-------------------|
| Node    | `bitcoind`     | `v31.0`   | `bitcoind`   | Yes             |                   |
| Node    | `utreexod`     | `v0.6.0`  | `utreexod`   | Yes             |                   |
|         |                |           |              |                 |                   |
| Indexer | `electrs`      | `v0.11.1` | `electrs`    | No              |                   |
| Indexer | `electrumx`    | `v1.20.0` | `electrumx`  | No              | Needs Python 3.10 |

Binaries are downloaded automatically at build time: see [`build.rs`](./build.rs).

### BitcoinD

```rs
use std::path::PathBuf;

use halfin::bitcoind::BitcoinD;
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
use halfin::bitcoind::BitcoinD;
use halfin::electrsd::ElectrsD;

let bitcoind = BitcoinD::new().unwrap();
bitcoind.generate(100).unwrap();

let electrs = ElectrsD::new(&bitcoind).unwrap();
electrs.wait_until_caught_up(&bitcoind, None).unwrap();
```

### UtreexoD

```rust
use halfin::utreexod::UtreexoD;

// Use a downloaded binary
let utreexod = UtreexoD::new().unwrap();

// Mine blocks
utreexod.generate(100).unwrap();
assert_eq!(utreexod.get_chain_tip().unwrap(), 100);

// Perform a raw RPC call
let res = utreexod.call("uptime", &[]).unwrap();
```

## Typed Node Configuration

Options shared by Bitcoin Core and `utreexod` use `NodeArgs` through each
configuration's `args` field. Daemon-specific options use `BitcoinDArgs` and
`UtreexoDArgs`, while `raw_args` remains available for options `halfin` does
not model:

```rust
use halfin::bitcoin::{FeeRate, Network};
use halfin::bitcoind::{BitcoinD, BitcoinDConf};
use halfin::node::PruneMode;
use halfin::utreexod::{UtreexoD, UtreexoDConf};

let mut bitcoind_conf = BitcoinDConf::default();
bitcoind_conf.args.network = Network::Signet;
bitcoind_conf.args.txindex = false;
bitcoind_conf.args.prune = PruneMode::Automatic(1_024);
bitcoind_conf.bitcoind_args.fallback_fee_rate = FeeRate::from_sat_per_vb_u32(2);
bitcoind_conf.raw_args.push("-debug=net".to_string());
let bitcoind = BitcoinD::new_with_conf(&bitcoind_conf).unwrap();

let mut utreexod_conf = UtreexoDConf::default();
utreexod_conf.args.txindex = true;
utreexod_conf.utreexo_args.proof_index_max_memory_mib = 512;
utreexod_conf.utreexo_args.dns_seed = true;
utreexod_conf
    .raw_args
    .push("--debuglevel=info".to_string());
let utreexod = UtreexoD::new_with_conf(&utreexod_conf).unwrap();
```

Raw arguments that duplicate typed settings are rejected. For example,
`bitcoind_conf.raw_args.push("-txindex=0".to_string())` conflicts with
`bitcoind_conf.args.txindex`; mutate the typed field instead.

## Indexer Configuration

`ElectrsD` and `ElectrumxD` accept an `&impl Node`, so their network and RPC
authentication are derived from the backing node. Of the bundled nodes, only
`BitcoinD` is currently supported. `UtreexoD` is rejected until its
indexer-facing P2P and RPC compatibility issues are fixed; the Electrs P2P
issue is documented [here](docs/utreexod-electrs-p2p-bug.md).
ElectrumX-specific options use `ElectrumxDArgs`, while `raw_args` remains
available for unmodeled options:

```rust
use halfin::bitcoind::BitcoinD;
use halfin::electrsd::{ElectrsD, ElectrsDConf};
use halfin::electrumxd::{ElectrumxD, ElectrumxDConf};
use halfin::indexer::Indexer;

let mut electrsd_conf = ElectrsDConf::default();
electrsd_conf
    .raw_args
    .push("--log-filters=debug".to_string());
let bitcoind = BitcoinD::new().unwrap();
let electrsd = ElectrsD::new_with_conf(&bitcoind, &electrsd_conf).unwrap();

let mut electrumxd_conf = ElectrumxDConf::default();
electrumxd_conf.electrumx_args.coin = "Bitcoin".to_string();
let electrumxd = ElectrumxD::new_with_conf(&bitcoind, &electrumxd_conf).unwrap();

fn electrum_url<I: Indexer>(indexer: &I) -> String {
    indexer.electrum_url()
}

assert_eq!(electrum_url(&electrsd), electrsd.electrum_url());
assert_eq!(electrum_url(&electrumxd), electrumxd.electrum_url());
```

The BitcoinD backing node must be unpruned for `electrs` and must have
`txindex` enabled for ElectrumX. Raw arguments cannot override the
network, RPC authentication, addresses, or directories owned by `halfin`; for
example, `electrsd_conf.raw_args.push("--network=signet".to_string())` is
rejected.
Configure the network through the backing node's `args` instead.

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
