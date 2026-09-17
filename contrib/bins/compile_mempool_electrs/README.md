# mempool/electrs binary builder

Use this Cargo example to build `mempool/electrs` archives for `halfin`.
The builder uses the `v3.3.0` tag.

The builder is a Cargo example. It uses `xshell` from `dev-dependencies`.

## Prerequisites

Run this builder on an Apple Silicon Mac. The builder uses `cargo build` for macOS and `cross` for Linux. It needs macOS
to build the macOS archives.

The upstream `rust-toolchain` file selects Rust 1.87. Rustup selects this
toolchain in the source directory. The builder installs its target triples.
It removes the parent process's `RUSTUP_TOOLCHAIN` setting so child commands
use the upstream file.

Install the Rust build helpers:

```sh
cargo install cross
```

Start Docker or Podman before you run the builder. `cross` uses a container
for each Linux target. The builder checks the container engine before use.

## Usage

From the repository root, run:

```sh
just compile-bins compile-mempool-electrs
```

The builder uses existing archives on later runs. To rebuild all targets, run:

```sh
cargo run --example compile-mempool-electrs -- --force
```

The script checks out upstream tag `v3.3.0` under:

```text
contrib/bins/compile_mempool_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_mempool_electrs/dist/mempool-electrs-3.3.0/
```

Output files:

```text
mempool-electrs-darwin-amd64.tar.gz
mempool-electrs-darwin-arm64.tar.gz
mempool-electrs-linux-amd64.tar.gz
mempool-electrs-linux-arm64.tar.gz
mempool-electrs-3.3.0-SHA256SUMS
```

Upload the archives to `indexer/mempool_electrs/mempool-electrs-3.3.0/`
on both mirrors. Copy the checksum file to
`sha256/indexer/mempool_electrs/` in this repository.

## Notes

The builder makes the `electrs` executable from the `mempool/electrs`
repository. Each archive name starts with `mempool-electrs-`. The executable
in each archive has the name `electrs`.

This source uses Unix networking APIs. The builder does not make Windows archives.

The Linux builds use `Cross.toml` from this directory. The container setup
links libclang to `/opt/halfin/libclang`. The builder sets `LIBCLANG_PATH` to
this path. It also sets `CLANG_PATH=/usr/bin/clang` for RocksDB bindings.

The checkout and Cargo cache remain under `tmp/` so later runs can reuse
compiled dependencies. The script checks out the pinned tag on each run but
does not clean `target/`.

On Apple Silicon, the builder sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross`. Docker Desktop can need Rosetta or amd64 emulation for these containers.
