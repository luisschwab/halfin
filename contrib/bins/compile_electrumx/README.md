# ElectrumX binary builder

This directory contains the local builder for `ElectrumX` launcher binaries
that `halfin` can later download at build time.

The builder is a Cargo example so its scripting dependency, `xshell`, stays in
`dev-dependencies`.

## Prerequisites

Run this builder from a machine with `uv`, `cross`, `cargo-xwin`, `zig`, and
`cmake`. The script creates a local `uv`-managed CPython 3.10 virtualenv under
`tmp/build-venv`, builds the ElectrumX wheel from the pinned GitHub tag, and
uses that venv's `pip download --platform` to resolve wheels for each target
platform's CPython ABI. It then embeds that wheelhouse into a small Rust
launcher executable for each target, using the same Cargo/Cross/cargo-xwin split
as the `romanz/electrs` builder.

Install `uv` if it is missing:

```sh
cargo install --git https://github.com/astral-sh/uv uv
```

The generated binaries still require a compatible Python interpreter on the
machine that runs ElectrumX. The compiled launcher extracts its embedded
wheelhouse, creates a private virtualenv on first use, then runs
`electrumx_server`.
macOS and Linux launchers default to `python3.10`, Windows x86_64 defaults to
`py -3.10`, and Windows ARM64 defaults to `py -3.11`. Set
`PYTHON=/path/to/python` when the default command is not on `PATH`.

`plyvel` does not publish wheels for the target matrix. macOS and Linux
wheelhouses include the `plyvel` source distribution, so the target machine
needs LevelDB development headers and a compiler the first time the launcher
creates its private virtualenv. Windows wheelhouses include `plyvel` wheels
cross-built locally by this builder using `zig`, CMake, LevelDB, and
`python-build-standalone` headers/import libraries.

## Usage

From the repository root:

```sh
cargo run --example cross-compile-electrumx
```

Existing archives are skipped on later runs. To rebuild and repackage every
target:

```sh
cargo run --example cross-compile-electrumx -- --force
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

Generated files:

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

Upload those files to the web server location that future download code will
use for `ElectrumX` downloads.

## Notes

ElectrumX is a Python project, so the published binary is a compiled launcher
with an embedded wheelhouse rather than the upstream Python application itself.
This builder mirrors the `romanz/electrs` cross-building flow: pinned upstream
checkout, single-file target artifacts, local `tmp/` build cache, Cargo for
macOS, Cross for Linux, cargo-xwin for Windows, `dist/` output, `--force`,
archive verification, and `SHA256SUMS`.

The upstream project declares Unix support. Windows x86_64 and aarch64 bundles
are supported by cross-building the native `plyvel` extension locally, because
`plyvel` does not publish compatible Windows wheels on PyPI. Windows x86_64
uses CPython 3.10; Windows ARM64 uses CPython 3.11 because the standalone
CPython provider does not publish Windows ARM64 CPython 3.10 archives.
