# halfin

> A bitcoin node runner 🏃‍♂️

This crate makes it simple to run regtest `bitcoind` and `utreexod` instances from Rust code
in integration test contexts. Pretty much [`corepc_node`](https://crates.io/crates/corepc-node) 
with `utreexod` support.

## Supported Implementations

| Feature           | Implementation | Version |
|-------------------|----------------|---------|
| `bitcoind_30_2`   | `bitcoind`     | v30.2   |
| `utreexod_0_5_0`  | `utreexod`     | v0.5.0  |

Both features are enabled by default. Binaries are downloaded automatically at build time, see [`build.rs`](./build.rs).

## BitcoinD

```rs
use halfin::bitcoind::BitcoinD;

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

let node = UtreexoD::download_new().unwrap();

node.generate(10).unwrap();
assert_eq!(node.get_height().unwrap(), 10);
```

## Developing

This project uses [`cargo-rbmt`](https://github.com/rust-bitcoin/rust-bitcoin-maintainer-tools/tree/master/cargo-rbmt)
to manage everything related to `cargo`, such as formatting, linting, testing and CI. To install them, run:

```console
~$ cargo install cargo-rbmt
```

A `justfile` is provided for convenience. Run `just` to see available commands:

```console
~$ just
> halfin
> A regtest runner for `bitcoind` and `utreexod`

Available recipes:
    build      # Build `halfin` [alias: b]
    check      # Check code formatting, compilation, and linting [alias: c]
    check-sigs # Checks whether all commits in this branch are signed [alias: cs]
    doc        # Generate documentation [alias: d]
    doc-open   # Generate and open documentation [alias: do]
    fmt        # Format code [alias: f]
    lock       # Regenerate Cargo-recent.lock and Cargo-minimal.lock [alias: l]
    pre-push   # Run pre-push suite: lock, fmt, check, and test [alias: p]
    test       # Run tests across all toolchains and lockfiles [alias: t]
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
