# Blockstream/electrs binary builder

This directory contains the local builder for the `Blockstream/electrs` binaries
that `halfin` can download at build time. The builder pins
`Blockstream/electrs` commit `4b1a018`.

The builder is a Cargo example so its scripting dependency, `xshell`, stays in
`dev-dependencies`.

## Prerequisites

Run this builder from an Apple Silicon macOS host. The script builds macOS
artifacts with `cargo build` and Linux artifacts with `cross`. It does not
provide a non-macOS path for producing the macOS release archives.

The pinned checkout selects Rust 1.92.0 through `rust-toolchain.toml`.
Install that toolchain and its target triples with rustup before building.
The builder uses upstream `Cargo.lock` with `--locked`.

Install the Rust build helpers:

```sh
cargo install cross
```

Start Docker or Podman before running the builder. `cross` uses a container
engine for the Linux targets, and the script selects an engine only after
`docker info` or `podman info` succeeds.

## Usage

From the repository root:

```sh
just compile-bins cross-compile-blockstream-electrs
```

Existing archives are skipped on later runs. To rebuild and repackage every
target:

```sh
cargo run --example cross-compile-blockstream-electrs -- --force
```

The script checks out and verifies the full upstream commit hash under:

```text
contrib/bins/compile_blockstream_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_blockstream_electrs/dist/blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b/
```

Generated files:

```text
blockstream-electrs-darwin-amd64.tar.gz
blockstream-electrs-darwin-arm64.tar.gz
blockstream-electrs-linux-amd64.tar.gz
blockstream-electrs-linux-arm64.tar.gz
blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b-SHA256SUMS
```

Upload those files to the `blockstream_electrs/blockstream-electrs-4b1a0186b12ae3e0ef6697a547d53f3a93d9c66b/`
directory on each binary mirror. Copy the generated checksum file to
`sha256/indexer/blockstream_electrs/` in this repository.

## Notes

The builder follows the upstream README: it builds the `electrs` Cargo binary
from the `Blockstream/electrs` repository. The archives use a
`blockstream-electrs-*` prefix to keep them distinct from the upstream
`romanz/electrs` artifacts, but the executable inside each archive remains
`electrs`.

The pinned upstream release uses Unix-only networking APIs, so the builder does
not produce native Windows binaries.

The Linux builds use `Cross.toml` from this directory. The image setup links its
distro-provided libclang into `/opt/halfin/libclang`, and the builder passes that stable
`LIBCLANG_PATH` plus `CLANG_PATH=/usr/bin/clang` because RocksDB's bindings require libclang.

The checkout and Cargo cache remain under `tmp/` so later runs can reuse
compiled dependencies. The script checks out the pinned commit on each run but
does not clean `target/`.

On Apple Silicon, the script sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross` builds. Docker Desktop can require Rosetta or amd64 emulation for
those Linux containers.
