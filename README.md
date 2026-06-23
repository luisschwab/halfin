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

> A {regtest} bitcoin node runner 🏃‍♂️

This crate makes it simple to run regtest [`bitcoind`](https://github.com/bitcoin/bitcoin),
[`utreexod`](https://github.com/utreexo/utreexod), and [`electrs`](https://github.com/romanz/electrs)
instances from Rust code, useful in integration test contexts.

_Heavily_ inspired by the [`bitcoind`](https://crates.io/crates/bitcoind)
and [`electrsd`](https://crates.io/crates/electrsd) crates.

## Supported Implementations

| Implementation | Version   | Feature Flag     | Default Feature |
|----------------|-----------|------------------|-----------------|
| `bitcoind`     | `v31.0`   | `bitcoind_31_0`  | Yes             |
|                |           |                  |                 |
| `electrs`      | `v0.11.1` | `electrs_0_11_1` | No              |
|                |           |                  |                 |
| `utreexod`     | `v0.5.2`  | `utreexod_0_5_2` | Yes             |

Binaries are downloaded automatically at build time: see [`build.rs`](./build.rs).

### BitcoinD

```rs
use halfin::connect;
use halfin::bitcoind::BitcoinD;

// Use a downloaded binary
let bitcoind_alpha = BitcoinD::new().unwrap();

// Use a local binary
let bin_path = PathBuf::from_str("/usr/local/bin/bitcoind").unwrap();
let bitcoind_beta = BitcoinD::from_bin(&bin_path).unwrap();

// Connect peers
connect(&bitcoind_alpha, &bitcoind_beta).unwrap()

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
~$ just
> halfin
> A regtest runner for `bitcoind` and `utreexod`

Available recipes:
    audit      # Run `cargo audit` on all lockfiles and prune ignored advisories [alias: a]
    build      # Build `halfin` [alias: b]
    check      # Check code formatting, compilation, and linting [alias: c]
    check-sigs # Checks whether all commits in this branch are signed [alias: cs]
    doc        # Generate documentation [alias: d]
    doc-open   # Generate and open documentation [alias: do]
    fmt        # Format code [alias: f]
    lock       # Regenerate Cargo-recent.lock and Cargo-minimal.lock [alias: l]
    pre-push   # Run pre-push checks [alias: p]
    shellcheck # Run ShellCheck [alias: sc]
    test       # Run tests with relevant toolchain and lockfile combinations [alias: t]
    toolchains # Install and/or Update `cargo-rbmt` and Stable and Nightly toolchains
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
