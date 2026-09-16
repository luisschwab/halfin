# Package Frigate

This Cargo example turns the [Frigate v1.5.3 release assets](https://github.com/sparrowwallet/frigate/releases/tag/1.5.3)
into the four archives that `halfin` expects. It packages the complete application images, including their
Java runtimes. It does not compile Frigate from source, install software, or upload files.

## Requirements

Run the recipe on macOS with `just`, `curl`, `tar`, `shasum`, `find`, `codesign`, and 7-Zip's `7zz` available.
The macOS images are signed during packaging, so this recipe requires macOS. Set `FRIGATE_7ZZ` to the path on
`7zz` if it is not on `PATH`.

```sh
just compile-bins package-frigate
```

## What it does

1. Downloads the two macOS DMGs and two Linux application archives from the pinned release into
   `contrib/bins/package_frigate/tmp/`. Existing downloads are reused.
2. Verifies each downloaded asset against its upstream SHA-256 checksum embedded in the example.
3. Extracts each complete application image. For macOS, it restores Java's legal-document symlinks
   that 7-Zip rejects, removes Apple metadata files, and signs the app bundle ad hoc. For Linux,
   it preserves the `frigate/` application directory.
4. Writes four normalized `.tar.gz` archives and `frigate-1.5.3-SHA256SUMS` under `frigate/frigate-1.5.3/`
   at the repository root.

| Platform     | Output archive                  | Launcher inside archive              |
|--------------|---------------------------------|--------------------------------------|
| macOS ARM64  | `frigate-darwin-arm64.tar.gz`   | `Frigate.app/Contents/MacOS/Frigate` |
| macOS x86_64 | `frigate-darwin-amd64.tar.gz`   | `Frigate.app/Contents/MacOS/Frigate` |
| Linux ARM64  | `frigate-linux-arm64.tar.gz`    | `frigate/bin/frigate`                |
| Linux x86_64 | `frigate-linux-amd64.tar.gz`    | `frigate/bin/frigate`                |

The `tmp/` and root `frigate/` directories are ignored by Git. Copy the generated checksum file
to `sha256/indexer/frigate/frigate-1.5.3-SHA256SUMS` when the archive contents change. Upload the
four archives to `indexer/frigate/frigate-1.5.3/` on both configured binary mirrors.

`build.rs` downloads the selected archive from the configured binary mirrors and verifies it
against the committed checksum file.
