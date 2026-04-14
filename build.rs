// SPDX-License-Identifier: MIT OR Apache-2.0

fn main() {
    bitcoind::download().unwrap();
    utreexod::download().unwrap();
}

/// Downloads  and verifies the `bitcoind` binary based on the enabled version feature.
///
/// Binaries are verified agains the corresponding SHA256SUM under `sha256/bitcoind`.
///
/// If the binary was previously dowloaded and exists under `target/bin/bitcoin`, it won't download again.
mod bitcoind {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Read;

    use anyhow::Context;
    use bitcoin_hashes::hex::FromHex;
    use bitcoin_hashes::sha256;
    use flate2::read::GzDecoder;
    use tar::Archive;

    include!("src/bitcoind/versions.rs");

    /// Return the platform-specific tarball filename for this version of `bitcoind`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return format!("bitcoin-{}-x86_64-apple-darwin.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return format!("bitcoin-{}-arm64-apple-darwin.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return format!("bitcoin-{}-x86_64-linux-gnu.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return format!("bitcoin-{}-aarch64-linux-gnu.tar.gz", BITCOIND_VERSION);
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Look up the expected SHA-256 hash for `filename` from the bundled
    /// `SHA256SUMS` file.
    ///
    /// Panics if the filename is not found in the checksum file.
    #[allow(clippy::lines_filter_map_ok)]
    fn get_expected_sha256(filename: &str) -> anyhow::Result<sha256::Hash> {
        let sha256sums_filename = format!(
            "sha256/bitcoind/bitcoin-core-{}-SHA256SUMS",
            BITCOIND_VERSION
        );
        let file = File::open(&sha256sums_filename)
            .with_context(|| format!("cannot find {:?}", sha256sums_filename))?;
        for line in BufReader::new(file).lines().flatten() {
            let tokens: Vec<_> = line.split("  ").collect();
            if tokens.len() == 2 && filename == tokens[1] {
                let bytes = <[u8; 32]>::from_hex(tokens[0]).unwrap();
                return Ok(sha256::Hash::from_byte_array(bytes));
            }
        }
        panic!(
            "Couldn't find hash for `{}` in `{}`:\n{}",
            filename,
            sha256sums_filename,
            std::fs::read_to_string(&sha256sums_filename).unwrap()
        );
    }

    /// Download, verify, and extract the `bitcoind` binary into
    /// `<CARGO_MANIFEST_DIR>/target/bin/bitcoin-<VERSION>/bitcoind`.
    ///
    /// Skips the download if the binary already exists.  The download
    /// endpoint can be overridden with the `BITCOIND_DOWNLOAD_ENDPOINT`
    /// environment variable; a local tarball can be used instead by setting
    /// `BITCOIND_TARBALL_FILE`.
    pub(crate) fn download() -> anyhow::Result<()> {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let download_dir = std::path::PathBuf::from(manifest_dir)
            .join("target")
            .join("bin");

        std::fs::create_dir_all(&download_dir)
            .with_context(|| format!("cannot create dir {:?}", download_dir))?;

        let existing_filename = download_dir
            .join(format!("bitcoin-{}", BITCOIND_VERSION))
            .join("bitcoind");

        if existing_filename.exists() {
            return Ok(());
        }

        let download_filename = get_download_filename();
        let expected_hash = get_expected_sha256(&download_filename)?;

        println!(
            "cargo:warning=Downloading bitcoind {}, this can take a while...",
            download_filename
        );

        let (file_or_url, tarball_bytes) = match std::env::var("BITCOIND_TARBALL_FILE") {
            Err(_) => {
                let endpoint = std::env::var("BITCOIND_DOWNLOAD_ENDPOINT")
                    .unwrap_or_else(|_| "https://bitcoincore.org/bin".to_owned());
                let url = format!(
                    "{}/bitcoin-core-{}/{}",
                    endpoint, BITCOIND_VERSION, download_filename
                );
                let resp = bitreq::get(&url)
                    .send()
                    .with_context(|| format!("cannot reach url {}", url))?;
                assert_eq!(resp.status_code, 200, "url {} didn't return 200", url);
                (url, resp.as_bytes().to_vec())
            }
            Ok(path) => {
                let f = File::open(&path)
                    .with_context(|| format!("cannot find {:?} (BITCOIND_TARBALL_FILE)", path))?;
                let mut buf = Vec::new();
                BufReader::new(f).read_to_end(&mut buf)?;
                (path, buf)
            }
        };

        let tarball_hash = sha256::Hash::hash(&tarball_bytes);
        assert_eq!(
            expected_hash, tarball_hash,
            "SHA-256 mismatch for {}",
            file_or_url
        );

        let dest_dir = download_dir.join(format!("bitcoin-{}", BITCOIND_VERSION));
        std::fs::create_dir_all(&dest_dir)
            .with_context(|| format!("cannot create dir {:?}", dest_dir))?;

        let d = GzDecoder::new(&tarball_bytes[..]);
        let mut archive = Archive::new(d);
        for mut entry in archive.entries().unwrap().flatten() {
            if let Ok(path) = entry.path() {
                if path.file_name() == Some(OsStr::new("bitcoind")) {
                    let dest = dest_dir.join("bitcoind");
                    let mut outfile = std::fs::File::create(&dest)
                        .with_context(|| format!("cannot create file {:?}", dest))?;
                    io::copy(&mut entry, &mut outfile).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = outfile.metadata().unwrap().permissions();
                        perms.set_mode(0o755);
                        outfile.set_permissions(perms).unwrap();
                    }
                    break;
                }
            }
        }

        // On `arm64` macOS the extracted binary must be locally code-signed before the OS will allow it to execute.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::process::Command;

            let signing_status = Command::new("codesign")
                .arg("-v")
                .arg(&existing_filename)
                .status()
                .with_context(|| "failed to verify bitcoind code signature")?;

            if !signing_status.success() {
                let status = Command::new("codesign")
                    .arg("-s")
                    .arg("-")
                    .arg(&existing_filename)
                    .status()
                    .with_context(|| "failed to sign bitcoind")?;
                if !status.success() {
                    return Err(anyhow::anyhow!(
                        "codesign failed with exit code {}",
                        status.code().unwrap_or(-1)
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Downloads and verifies the `utreexod` binary based on the enabled version feature.
///
/// Binaries are verified agains the corresponding SHA256SUM under `sha256/utreexod`.
///
/// If the binary was previously dowloaded and exists under `target/bin/utreexod`, it won't download again.
mod utreexod {
    use std::ffi::OsStr;
    use std::fs;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::path::PathBuf;

    use anyhow::Context;
    use bitcoin_hashes::hex::FromHex;
    use bitcoin_hashes::sha256;
    use flate2::read::GzDecoder;
    use tar::Archive;

    include!("src/utreexod/versions.rs");

    /// Return the platform-specific tarball filename for this version of `utreexod`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return format!("utreexod-darwin-amd64-{}-01.tar.gz", UTREEXOD_RELEASE_DATE);
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return format!("utreexod-darwin-arm64-{}-01.tar.gz", UTREEXOD_RELEASE_DATE);
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return format!("utreexod-linux-amd64-{}-01.tar.gz", UTREEXOD_RELEASE_DATE);
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return format!("utreexod-linux-arm64-{}-01.tar.gz", UTREEXOD_RELEASE_DATE);
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Look up the expected SHA-256 hash for `filename` from the bundled
    /// `SHA256SUMS` file.
    ///
    /// Panics if the filename is not found in the checksum file.
    #[allow(clippy::lines_filter_map_ok)]
    fn get_expected_sha256(filename: &str) -> anyhow::Result<sha256::Hash> {
        let sha256sums_filename =
            format!("sha256/utreexod/utreexod-{}-SHA256SUMS", UTREEXOD_VERSION);
        let file = File::open(&sha256sums_filename)
            .with_context(|| format!("cannot find {:?}", sha256sums_filename))?;
        for line in BufReader::new(file).lines().flatten() {
            let tokens: Vec<_> = line.split("  ").collect();
            if tokens.len() == 2 && filename == tokens[1] {
                let bytes = <[u8; 32]>::from_hex(tokens[0]).unwrap();
                return Ok(sha256::Hash::from_byte_array(bytes));
            }
        }
        panic!(
            "Couldn't find hash for `{}` in `{}`:\n{}",
            filename,
            sha256sums_filename,
            fs::read_to_string(&sha256sums_filename).unwrap()
        );
    }

    /// Download, verify, and extract the `utreexod` binary into
    /// `<CARGO_MANIFEST_DIR>/target/bin/utreexod-<VERSION>/utreexod`.
    ///
    /// Skips the download if the binary already exists. The download
    /// endpoint can be overridden with the `UTREEXOD_DOWNLOAD_ENDPOINT`
    /// environment variable.
    pub(crate) fn download() -> anyhow::Result<()> {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let download_dir = PathBuf::from(manifest_dir).join("target").join("bin");

        fs::create_dir_all(&download_dir)
            .with_context(|| format!("cannot create dir {:?}", download_dir))?;

        let existing_filename = {
            let mut p = download_dir.join(format!("utreexod-{}", UTREEXOD_VERSION));
            if cfg!(target_os = "windows") {
                p.push("utreexod.exe");
            } else {
                p.push("utreexod");
            }
            p
        };

        if existing_filename.exists() {
            return Ok(());
        }

        let download_filename = get_download_filename();
        let expected_hash = get_expected_sha256(&download_filename)?;

        println!(
            "cargo:warning=Downloading utreexod {}, this can take a while...",
            download_filename
        );

        let endpoint = std::env::var("UTREEXOD_DOWNLOAD_ENDPOINT").unwrap_or_else(|_| {
            format!(
                "https://github.com/utreexo/utreexod/releases/download/v{}",
                UTREEXOD_VERSION
            )
        });
        let url = format!("{}/{}", endpoint, download_filename);
        let resp = bitreq::get(&url)
            .send()
            .with_context(|| format!("cannot reach url {}", url))?;
        assert_eq!(resp.status_code, 200, "url {} didn't return 200", url);

        let tarball_bytes = resp.as_bytes().to_vec();
        let tarball_hash = sha256::Hash::hash(&tarball_bytes);
        assert_eq!(expected_hash, tarball_hash, "SHA-256 mismatch for {}", url);

        let dest_dir = download_dir.join(format!("utreexod-{}", UTREEXOD_VERSION));
        fs::create_dir_all(&dest_dir)
            .with_context(|| format!("cannot create dir {:?}", dest_dir))?;

        let d = GzDecoder::new(&tarball_bytes[..]);
        let mut archive = Archive::new(d);
        for mut entry in archive.entries().unwrap().flatten() {
            if let Ok(path) = entry.path() {
                if path.file_name() == Some(OsStr::new("utreexod")) {
                    let dest = dest_dir.join("utreexod");
                    let mut outfile = fs::File::create(&dest)
                        .with_context(|| format!("cannot create file {:?}", dest))?;
                    io::copy(&mut entry, &mut outfile).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = outfile.metadata().unwrap().permissions();
                        perms.set_mode(0o755);
                        outfile.set_permissions(perms).unwrap();
                    }
                    break;
                }
            }
        }

        // On `arm64` macOS the extracted binary must be locally code-signed before the OS will allow it to execute.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::process::Command;

            let signing_status = Command::new("codesign")
                .arg("-v")
                .arg(&existing_filename)
                .status()
                .with_context(|| "failed to verify utreexod code signature")?;

            if !signing_status.success() {
                let status = Command::new("codesign")
                    .arg("-s")
                    .arg("-")
                    .arg(&existing_filename)
                    .status()
                    .with_context(|| "failed to sign utreexod")?;
                if !status.success() {
                    return Err(anyhow::anyhow!(
                        "codesign failed with exit code {}",
                        status.code().unwrap_or(-1)
                    ));
                }
            }
        }

        Ok(())
    }
}
