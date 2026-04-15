# halfin

<p>
    <a href="https://crates.io/crates/halfin"><img src="https://img.shields.io/crates/v/halfin.svg"/></a>
    <a href="https://docs.rs/halfin"><img src="https://img.shields.io/badge/docs.rs-halfin-green"/></a>
    <a href="https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/"><img src="https://img.shields.io/badge/rustc-1.85.0%2B-orange.svg?label=MSRV"/></a>
    <a href="https://github.com/luisschwab/halfin/blob/master/LICENSE"><img src="https://img.shields.io/badge/License-MIT%2FApache--2.0-red.svg"/></a>
    <a href="https://github.com/luisschwab/halfin/actions/workflows/rust.yml"><img src="https://github.com/luisschwab/halfin/actions/workflows/rust.yml/badge.svg"></a>
</p>

> A (regtest) bitcoin node runner 🏃‍♂️

This crate makes it simple to run regtest [`bitcoind`](https://github.com/bitcoin/bitcoin)
and [`utreexod`](https://github.com/utreexo/utreexod) instances from Rust code, useful in
integration testing contexts.

Pretty much [`corepc_node`](https://crates.io/crates/corepc-node)
with [`utreexod`](https://github.com/utreexo/utreexod) support.

## Supported Implementations and Versions

| Implementation | Version  | Feature Flag     |
|----------------|----------|----------------- |
| `bitcoind`     | `v30.2`  | `bitcoind_30_2`  |
|                |          |                  |
| `utreexod`     | `v0.5.0` | `utreexod_0_5_0` |

By default, the `bitcoind_30_2` and `utreexod_0_5_0` features are enabled.

Binaries are downloaded automatically at build time: see [`build.rs`](./build.rs).

## BitcoinD

```rs
use halfin::bitcoind::BitcoinD;

// Use downloaded/cached binaries
let bitcoind_alpha = BitcoinD::download_new().unwrap();
let bitcoind_beta = BitcoinD::download_new().unwrap();

bitcoind_alpha.add_peer(bitcoind_beta.get_p2p_socket()).unwrap();

bitcoind_alpha.generate(10).unwrap();
assert_eq!(bitcoind_alpha.get_height().unwrap(), 10);
assert_eq!(bitcoind_beta.get_height().unwrap(), 10);
```

## UtreexoD

```rust
use halfin::utreexod::UtreexoD;

// Use a local binary by specifying its path
let bin_path = PathBuf::from_str("/usr/local/bin/utreexod").unwrap();
let utreexod = UtreexoD::from_bin(utreexod_bin: &bin_path).unwrap();

utreexod.generate(10).unwrap();
assert_eq!(utreexod.get_height().unwrap(), 10);
```

## Developing

This project uses [`just`](https://github.com/casey/just) for command running, and
[`cargo-rbmt`](https://github.com/rust-bitcoin/rust-bitcoin-maintainer-tools/tree/master/cargo-rbmt)
to manage everything related to `cargo`, such as formatting, linting, testing and CI. To install them, run:

```console
~$ cargo install just

~$ cargo install cargo-rbmt
```

A `justfile` is provided for convenience. Run `just` to see available commands:

```console
~$ just
> halfin
> A regtest runner for `bitcoind` and `utreexod`

Available recipes:
    build       # Build `halfin` [alias: b]
    check       # Check code formatting, compilation, and linting [alias: c]
    check-sigs  # Checks whether all commits in this branch are signed [alias: cs]
    delete-bins # Delete binaries under `target/bin/` [alias: db]
    doc         # Generate documentation [alias: d]
    doc-open    # Generate and open documentation [alias: do]
    fmt         # Format code [alias: f]
    lock        # Regenerate Cargo-recent.lock and Cargo-minimal.lock [alias: l]
    pre-push    # Run pre-push suite: lock, fmt, check, and test [alias: p]
    test        # Run tests across all toolchains and lockfiles [alias: t]
```


## Minimum Supported Rust Version (MSRV)

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
