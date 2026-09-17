# Blockstream/electrs binary builder

Use this Cargo example to build `Blockstream/electrs` archives for `halfin`.
The builder uses commit `4b1a018`.

The builder is a Cargo example. It uses `xshell` from `dev-dependencies`.

## Prerequisites

Run this builder on an Apple Silicon Mac. The builder uses `cargo build` for macOS and `cross` for Linux. It needs macOS
to build the macOS archives.

The upstream `rust-toolchain.toml` selects Rust 1.92.0. Install this toolchain
and its target triples before you build. The builder uses the upstream
`Cargo.lock` file and the `--locked` option.

Install the Rust build helpers:

```sh
cargo install cross
```

Start Docker or Podman before you run the builder. `cross` uses a container
for each Linux target. The builder checks the container engine before use.

## Usage

From the repository root, run:

```sh
just compile-bins compile-blockstream-electrs
```

The builder uses existing archives on later runs. To rebuild all targets, run:

```sh
cargo run --example compile-blockstream-electrs -- --force
```

The script checks out and verifies the full upstream commit hash under:

```text
contrib/bins/compile_blockstream_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_blockstream_electrs/dist/blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b/
```

Output files:

```text
blockstream-electrs-darwin-amd64.tar.gz
blockstream-electrs-darwin-arm64.tar.gz
blockstream-electrs-linux-amd64.tar.gz
blockstream-electrs-linux-arm64.tar.gz
blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b-SHA256SUMS
```

Upload the archives to `indexer/blockstream_electrs/blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b/`
on both mirrors. Copy the checksum file to
`sha256/indexer/blockstream_electrs/` in this repository.

## Notes

The builder makes the `electrs` executable from the `Blockstream/electrs`
repository. Each archive name starts with `blockstream-electrs-`. The
executable in each archive has the name `electrs`.

This source uses Unix networking APIs. The builder does not make Windows archives.

The Linux builds use `Cross.toml` from this directory. The container setup
links libclang to `/opt/halfin/libclang`. The builder sets `LIBCLANG_PATH` to
this path. It also sets `CLANG_PATH=/usr/bin/clang` for RocksDB bindings.

The builder keeps source files and Cargo build files under `tmp/`. It checks
the pinned commit on each run. It keeps compiled dependencies in `target/`.

On Apple Silicon, the builder sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross`. Docker Desktop can need Rosetta or amd64 emulation for these containers.
