# mempool/electrs binary builder

This directory contains the local builder for the `mempool/electrs` binaries
that `halfin` can download at build time. The builder pins the stable
`mempool/electrs` v3.3.0 tag.

The builder is a Cargo example so its scripting dependency, `xshell`, stays in
`dev-dependencies`.

## Prerequisites

Run this builder from an Apple Silicon macOS host. The script builds macOS
artifacts with `cargo build` and Linux artifacts with `cross`. It does not
provide a non-macOS path for producing the macOS release archives.

The upstream release pins Rust 1.87 in its `rust-toolchain` file. Rustup
selects that toolchain inside the checkout, and the builder installs all target
triples for that selected toolchain. The builder removes the parent Cargo
process's `RUSTUP_TOOLCHAIN` override from child commands so they honor the
upstream file.

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
cargo run --example cross-compile-mempool-electrs
```

Existing archives are skipped on later runs. To rebuild and repackage every
target:

```sh
cargo run --example cross-compile-mempool-electrs -- --force
```

The script checks out upstream tag `v3.3.0` under:

```text
contrib/bins/compile_mempool_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_mempool_electrs/dist/mempool-electrs-3.3.0/
```

Generated files:

```text
mempool-electrs-darwin-amd64.tar.gz
mempool-electrs-darwin-arm64.tar.gz
mempool-electrs-linux-amd64.tar.gz
mempool-electrs-linux-arm64.tar.gz
mempool-electrs-3.3.0-SHA256SUMS
```

Upload those files to the `mempool_electrs/mempool-electrs-3.3.0/`
directory on each binary mirror. Copy the generated checksum file to
`sha256/mempool_electrs/` in this repository.

## Notes

The builder follows the upstream README: it builds the `electrs` Cargo binary
from the `mempool/electrs` repository. The archives use a
`mempool-electrs-*` prefix to keep them distinct from the upstream
`romanz/electrs` artifacts, but the executable inside each archive remains
`electrs`.

The pinned upstream release uses Unix-only networking APIs, so the builder does
not produce native Windows binaries.

The Linux builds use `Cross.toml` from this directory. The image setup links its
distro-provided libclang into `/opt/halfin/libclang`, and the builder passes that stable
`LIBCLANG_PATH` plus `CLANG_PATH=/usr/bin/clang` because RocksDB's bindings require libclang.

The checkout and Cargo cache remain under `tmp/` so later runs can reuse
compiled dependencies. The script checks out the pinned tag on each run but
does not clean `target/`.

On Apple Silicon, the script sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross` builds. Docker Desktop can require Rosetta or amd64 emulation for
those Linux containers.
