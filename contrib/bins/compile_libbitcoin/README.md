# Build the libbitcoin-server archives

This Cargo example builds four `bs` archives:

| Operating system | Architecture |
|------------------|--------------|
| macOS            | ARM64        |
| macOS            | x86_64       |
| Linux            | ARM64        |
| Linux            | x86_64       |

Run the example on an Apple Silicon Mac. Install the prerequisites first.

## Prerequisites

1. Install the full Xcode application and Rosetta 2. The builder uses
   `/Applications/Xcode.app/Contents/Developer` if it exists.
2. Install these Homebrew packages: `autoconf automake libtool pkg-config`.
3. Start Docker Desktop. Enable ARM64 and amd64 Linux containers. Give Docker
   Desktop at least 16 GB of memory. The Linux builds use four compile jobs.

## Build the archives

From the repository root, run:

```sh
just compile-bins compile-libbitcoin
```

The example builds all four archives in one run. It uses existing archives if
you run it again. It puts the archives and the checksum file in
`contrib/bins/compile_libbitcoin/dist/libbitcoin-<full-commit>/`.
The checksum file has the name `libbitcoin-<full-commit>-SHA256SUMS`.

The builder uses `xshell` to run the build commands. It checks the full commit
hash for each libbitcoin source repository. Then it runs the upstream GNU build
script with release and static link settings. It builds Boost 1.86.0 and
bitcoin-core/secp256k1 v0.8.0 from source.

The builder uses Apple Clang for the macOS builds. The pinned source does not
compile with LLVM 23 libc++ on this Mac. For macOS x86_64, compiler wrappers
pass `-arch x86_64` to Apple Clang. The build script runs on ARM64.

The builder keeps its source and build files in the ignored `tmp/` directory.
The complete build can take several hours.

Before it makes an archive, the builder does these checks:

1. It runs `bs --version`, `bs --help`, and `bs --hardware` on the target
   architecture.
2. It checks the executable format and linked libraries.
3. It checks that the archive contains `bs`.

## Publish the archives

1. Upload all four archives to both mirrors. Use the directory
   `node/libbitcoin/libbitcoin-<full-commit>/` on each mirror.
2. Commit the checksum file in `sha256/node/libbitcoin/`.

When you enable the `libbitcoin` feature, `build.rs` downloads an archive from
one of the mirrors. It checks the archive against the checksum file.

The builder and the `libbitcoin` feature do not support Windows.
