# romanz/electrs binary builder

Use this Cargo example to build `romanz/electrs` archives for `halfin`.

The builder is a Cargo example. It uses `xshell` from `dev-dependencies`.

## Prerequisites

Run this builder on an Apple Silicon Mac. It uses `cargo build` for macOS,
`cross` for Linux, and `cargo-xwin` for Windows MSVC. It needs macOS to build
the macOS archives.

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

Windows builds use `cargo-xwin` instead of Docker or Podman. `cargo-xwin` uses
the Rust `llvm-tools` component and downloads Windows SDK metadata.

## Usage

From the repository root, run:

```sh
just compile-bins compile-romanz-electrs
```

The builder uses existing archives on later runs. To rebuild all targets, run:

```sh
cargo run --example compile-romanz-electrs -- --force
```

The script reads the upstream `romanz/electrs` release from
`src/indexer/romanz_electrsd/versions.rs` (currently `v0.12.0`).
It clones or updates the source tree under:

```text
contrib/bins/compile_romanz_electrs/tmp/electrs
```

It writes archives and checksums under:

```text
contrib/bins/compile_romanz_electrs/dist/electrs-0.12.0/
```

Output files:

```text
electrs-darwin-amd64.tar.gz
electrs-darwin-arm64.tar.gz
electrs-linux-amd64.tar.gz
electrs-linux-arm64.tar.gz
electrs-windows-amd64.zip
electrs-windows-arm64.zip
electrs-0.12.0-SHA256SUMS
```

Upload the archives to `indexer/romanz_electrs/electrs-0.12.0/` on both mirrors.
Copy the checksum file to `sha256/indexer/romanz_electrs/` in this repository.

## Notes

The Linux builds use `Cross.toml` from this directory. The Linux images
install `clang` and `libclang-dev` for RocksDB bindings. The builder finds
Clang in the container. It builds and links RocksDB from source as the
[upstream build instructions](https://github.com/romanz/electrs/blob/v0.12.0/doc/install.md)
describe. The executables still need the target system's C/C++ runtime libraries.

The builder keeps the source directory and Cargo build files under `tmp/`.
It checks out the pinned tag on each run. It keeps compiled dependencies in
`target/` for later runs.

On Apple Silicon, the builder sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` for
`cross`. Docker Desktop can need Rosetta or amd64 emulation for these containers.
