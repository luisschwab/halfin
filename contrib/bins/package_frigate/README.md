# Package Frigate

This Cargo example packages the [Frigate v1.5.3 release assets](https://github.com/sparrowwallet/frigate/releases/tag/1.5.3)
into four archives for `halfin`. Each archive contains the application and its Java runtime.
The example does not compile Frigate, install software, or upload files.

## Requirements

Run the recipe on macOS. Install `just`, `curl`, `tar`, `shasum`, `find`,
`codesign`, and 7-Zip's `7zz` first. The recipe signs the macOS images.
If `7zz` is not on `PATH`, set `FRIGATE_7ZZ` to its path.

```sh
just compile-bins package-frigate
```

## What it does

1. The example downloads two macOS DMGs and two Linux archives to
   `contrib/bins/package_frigate/tmp/`. It uses these files on later runs.
2. It checks each download against the upstream SHA-256 checksum in the example.
3. It extracts each application image. On macOS, it restores Java's legal-document
   symlinks, removes Apple metadata, and signs the app bundle. On Linux, it keeps
   the `frigate/` application directory.
4. It writes four `.tar.gz` archives and `frigate-1.5.3-SHA256SUMS` to
   `frigate/frigate-1.5.3/` at the repository root.

| Platform     | Output archive                  | Launcher inside archive              |
|--------------|---------------------------------|--------------------------------------|
| macOS ARM64  | `frigate-darwin-arm64.tar.gz`   | `Frigate.app/Contents/MacOS/Frigate` |
| macOS x86_64 | `frigate-darwin-amd64.tar.gz`   | `Frigate.app/Contents/MacOS/Frigate` |
| Linux ARM64  | `frigate-linux-arm64.tar.gz`    | `frigate/bin/frigate`                |
| Linux x86_64 | `frigate-linux-amd64.tar.gz`    | `frigate/bin/frigate`                |

Git ignores the `tmp/` and root `frigate/` directories. If archive contents
change, copy the checksum file to
`sha256/indexer/frigate/frigate-1.5.3-SHA256SUMS`. Upload all four archives
to `indexer/frigate/frigate-1.5.3/` on both mirrors.

`build.rs` downloads the archive for the target platform. It checks the
archive against the committed checksum file.
