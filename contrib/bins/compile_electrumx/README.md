# ElectrumX binary builder

Use this Cargo example to build `ElectrumX` launchers for `halfin`.

The builder is a Cargo example. It uses `xshell` from `dev-dependencies`.

## Prerequisites

Install `uv`, `cross`, `cargo-xwin`, `zig`, and `cmake` before you run the builder.
The builder creates a CPython 3.10 virtual environment in `tmp/build-venv`.
It builds the ElectrumX wheel from the pinned GitHub tag. It downloads the
Python wheels for each target platform. Then it puts those wheels in a Rust
launcher for each target.

Install `uv` if it is missing:

```sh
cargo install --git https://github.com/astral-sh/uv uv
```

The launchers need a compatible Python interpreter on the target machine.
On first use, a launcher extracts its Python wheels, creates a private virtual
environment, and starts `electrumx_server`.

- macOS and Linux use `python3.10` by default.
- Windows x86_64 uses `py -3.10` by default.
- Windows ARM64 uses `py -3.11` by default.

If the default command is not on `PATH`, set `PYTHON=/path/to/python`.

`plyvel` does not publish wheels for all targets. The macOS and Linux launchers
include its source archive. On first use, these targets need a compiler and
LevelDB development headers. The builder makes Windows `plyvel` wheels with
`zig`, CMake, LevelDB, and `python-build-standalone` files.

## Usage

From the repository root, run:

```sh
just compile-bins compile-electrumx
```

The builder uses existing archives on later runs. To rebuild all targets, run:

```sh
cargo run --example compile-electrumx -- --force
```

The script hardcodes upstream `spesmilo/electrumx` release tag `1.20.0`. It
clones or updates the source tree under:

```text
contrib/bins/compile_electrumx/tmp/electrumx
```

It writes archives and checksums under:

```text
contrib/bins/compile_electrumx/dist/electrumx-1.20.0/
```

Output files:

```text
electrumx-darwin-amd64.tar.gz
electrumx-darwin-arm64.tar.gz
electrumx-linux-amd64.tar.gz
electrumx-linux-arm64.tar.gz
electrumx-windows-amd64.zip
electrumx-windows-arm64.zip
electrumx-1.20.0-SHA256SUMS
```

Unix archives contain exactly one file:

```text
electrumx
```

Windows archives contain exactly one file:

```text
electrumx.exe
```

Upload the archives to `indexer/electrumx/electrumx-1.20.0/` on both mirrors.
Copy the checksum file to `sha256/indexer/electrumx/` in this repository.

## Notes

ElectrumX is a Python project. Each archive contains a Rust launcher with
Python wheels. The builder keeps source files and build files in `tmp/`.
It uses Cargo for macOS, `cross` for Linux, and `cargo-xwin` for Windows.
It checks each archive and writes a `SHA256SUMS` file.

The upstream project declares Unix support. This builder also makes Windows
launchers. It builds the native `plyvel` extension for Windows because PyPI
does not have compatible wheels. Windows x86_64 uses CPython 3.10. Windows
ARM64 uses CPython 3.11 because the standalone CPython provider does not
publish CPython 3.10 for that target.
