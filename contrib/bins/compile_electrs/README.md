# romanz/electrs binary builder

This directory contains the local builder for the `romanz/electrs` binaries that
`halfin` can later download at build time.

The builder is a Cargo example so its scripting dependency, `xshell`, stays in
`dev-dependencies`.

## Prerequisites

Run this builder from an Apple Silicon macOS host. The script builds macOS
artifacts with `cargo build`, Linux artifacts with `cross`, and Windows MSVC
artifacts with `cargo-xwin`; it does not provide a non-macOS path for producing
the macOS release archives.

Install the Rust build helpers:

```sh
cargo install cross
cargo install --locked cargo-xwin
```

Install LLVM tools for Windows MSVC cross-compilation:

```sh
rustup component add llvm-tools
```

On macOS, install LLVM if `cargo-xwin` cannot find a suitable toolchain:

```sh
brew install llvm
```

Start Docker or Podman before running the builder. `cross` uses a container
engine for the Linux targets, and the script only selects an engine after
`docker info` or `podman info` succeeds.

Windows targets do not use Docker or Podman. They are built with `cargo-xwin`,
which uses the Rust `llvm-tools` component and the Windows SDK metadata it
downloads.

## Usage

From the repository root:

```sh
cargo run --example cross-compile-electrs
```

Existing archives are skipped on later runs. To rebuild and repackage every
target:

```sh
cargo run --example cross-compile-electrs -- --force
```

The script reads the upstream `romanz/electrs` release from
`src/indexer/electrsd/versions.rs` (currently `v0.12.0`).
It clones or updates the source tree under:

```text
contrib/bins/compile_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_electrs/dist/electrs-0.12.0/
```

Generated files:

```text
electrs-darwin-amd64.tar.gz
electrs-darwin-arm64.tar.gz
electrs-linux-amd64.tar.gz
electrs-linux-arm64.tar.gz
electrs-windows-amd64.zip
electrs-windows-arm64.zip
electrs-0.12.0-SHA256SUMS
```

Upload those files to the web server location that `build.rs`
will later use for `romanz/electrs` downloads.

## Notes

The Linux builds use `Cross.toml` from this directory. The macOS builds use
`cargo build`, and the Windows MSVC builds use `cargo xwin build`.
The Linux images install `clang` and `libclang-dev` for the RocksDB bindings.
The build discovers Clang inside the container; host Clang paths are not forwarded.
RocksDB is built from source and linked statically, following the
[upstream build instructions](https://github.com/romanz/electrs/blob/v0.12.0/doc/install.md).
The resulting binaries still depend on the target system's C/C++ runtime libraries.

The `romanz/electrs` checkout and Cargo build cache are kept under `tmp/` so reruns can
reuse previously compiled dependencies. The script checks out the pinned tag on
each run, but it does not clean `target/`.

On Apple Silicon, the script sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross` builds. Docker Desktop may need Rosetta or amd64 emulation enabled for
those Linux container builds.
