// SPDX-License-Identifier: MIT OR Apache-2.0

fn main() {
    // Check if `bitcoind` is cached and download it if not.
    bitcoind::download();
    // Check if `utreexod` is cached and download it if not.
    utreexod::download();
}

/// Downloads  and verifies the `bitcoind` binary based on the enabled version feature.
///
/// Binaries are verified agains the corresponding SHA256SUM under `sha256/bitcoind`.
///
/// If the binary was previously dowloaded and exists under `target/bin/bitcoin`, it won't download again.
mod bitcoind {
    use std::env;
    use std::ffi::OsStr;
    use std::fs;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::path::PathBuf;

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

    /// Look up the expected SHA256 hash for `filename` from the bundled `SHA256SUMS` file.
    ///
    /// Panics if the filename is not found in the checksum file.
    #[allow(clippy::lines_filter_map_ok)]
    fn get_expected_sha256(bin_name: &str) -> sha256::Hash {
        let sha256sums_filename = format!(
            "sha256/bitcoind/bitcoin-core-{}-SHA256SUMS",
            BITCOIND_VERSION
        );
        let file = File::open(&sha256sums_filename)
            .map_err(|e| {
                format!(
                    "Cannot open `bitcoind` SHA256SUMS file={}: {:?}",
                    sha256sums_filename, e
                )
            })
            .unwrap();

        for line in BufReader::new(file).lines().flatten() {
            let tokens: Vec<_> = line.split("  ").collect();
            if tokens.len() == 2 && bin_name == tokens[1] {
                let bytes = <[u8; 32]>::from_hex(tokens[0]).unwrap();
                return sha256::Hash::from_byte_array(bytes);
            }
        }

        // Failed to get the expected SHA256SUM for the binary. Is it present in the file?
        panic!(
            "Failed to find SHA256SUM for binary={} at file={}",
            bin_name, sha256sums_filename
        );
    }

    /// Download, verify, and extract the `bitcoind` binary into
    /// `<CARGO_MANIFEST_DIR>/target/bin/bitcoin-<VERSION>/bitcoind`.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        const BITCOIND_DOWNLOAD_URL: &str = "https://bitcoincore.org";

        let manifest_directory = env::var("CARGO_MANIFEST_DIR").unwrap();
        let download_directory = PathBuf::from(manifest_directory).join("target").join("bin");

        fs::create_dir_all(&download_directory)
            .map_err(|e| {
                format!(
                    "Cannot create download directory at={}: {:?}",
                    download_directory.display(),
                    e
                )
            })
            .unwrap();

        let existing_filename = download_directory
            .join(format!("bitcoin-{}", BITCOIND_VERSION))
            .join("bitcoind");

        let download_filename = get_download_filename();
        let expected_hash = get_expected_sha256(&download_filename);

        if existing_filename.exists() {
            return;
        } else {
            println!(
                "cargo:warning=Downloading `bitcoind` {} from `bitcoincore.org`",
                download_filename
            );
        }

        let bitcoind_tarball_bytes = {
            let download_url = format!(
                "{}/bin/bitcoin-core-{}/{}",
                BITCOIND_DOWNLOAD_URL, BITCOIND_VERSION, download_filename
            );

            let response = bitreq::get(&download_url)
                .send()
                .map_err(|e| format!("Failed to GET {}: {:?}", download_url, e))
                .unwrap();

            assert_eq!(
                response.status_code, 200,
                "Failed to GET {}: {} {}",
                download_url, response.status_code, response.reason_phrase
            );

            let bitcoind_tarball = response.as_bytes().to_vec();

            bitcoind_tarball
        };

        let bitcoind_tarball_hash = sha256::Hash::hash(&bitcoind_tarball_bytes);
        assert_eq!(
            bitcoind_tarball_hash, expected_hash,
            "Downloaded bitcoind binary hash does not match expected hash: downloaded={} != expected={}",
            bitcoind_tarball_hash, expected_hash
        );

        let destination_directory =
            download_directory.join(format!("bitcoin-{}", BITCOIND_VERSION));
        fs::create_dir_all(&destination_directory)
            .map_err(|e| {
                format!(
                    "Cannot create destination directory={}: {}",
                    destination_directory.display(),
                    e
                )
            })
            .unwrap();

        let gz_decoder = GzDecoder::new(&bitcoind_tarball_bytes[..]);
        let mut archive = Archive::new(gz_decoder);

        for mut entry in archive.entries().unwrap().flatten() {
            if let Ok(path) = entry.path() {
                if path.file_name() == Some(OsStr::new("bitcoind")) {
                    let destination_path = destination_directory.join("bitcoind");
                    let mut output_file = File::create(&destination_path)
                        .map_err(|e| {
                            format!(
                                "Cannot create bitcoind file at destination path={}: {}",
                                destination_path.display(),
                                e
                            )
                        })
                        .unwrap();

                    io::copy(&mut entry, &mut output_file).unwrap();

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = output_file.metadata().unwrap().permissions();
                        perms.set_mode(0o755);
                        output_file.set_permissions(perms).unwrap();
                    }
                    break;
                }
            }
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::process::Command;

            let signing_status = Command::new("codesign")
                .arg("-v")
                .arg(&existing_filename)
                .status()
                .map_err(|e| format!("Failed to run `codesign -v` on `bitcoind`: {}", e))
                .unwrap();

            if !signing_status.success() {
                Command::new("codesign")
                    .arg("-s")
                    .arg("-")
                    .arg(&existing_filename)
                    .status()
                    .map_err(|e| format!("Failed to run `codesign -s` on `bitcoind`: {}", e))
                    .unwrap();
            }
        }
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

    /// Look up the expected SHA256 hash for `filename` from the bundled `SHA256SUMS` file.
    ///
    /// Panics if the filename is not found in the checksum file.
    #[allow(clippy::lines_filter_map_ok)]
    fn get_expected_sha256(bin_name: &str) -> sha256::Hash {
        let sha256sums_filename =
            format!("sha256/utreexod/utreexod-{}-SHA256SUMS", UTREEXOD_VERSION);

        let file = File::open(&sha256sums_filename)
            .map_err(|e| {
                format!(
                    "Cannot open `utreexod` SHA256SUMS file={}: {:?}",
                    sha256sums_filename, e
                )
            })
            .unwrap();

        for line in BufReader::new(file).lines().flatten() {
            let tokens: Vec<_> = line.split("  ").collect();
            if tokens.len() == 2 && bin_name == tokens[1] {
                let bytes = <[u8; 32]>::from_hex(tokens[0]).unwrap();
                return sha256::Hash::from_byte_array(bytes);
            }
        }

        // Failed to get the expected SHA256SUM for the binary. Is it present in the file?
        panic!(
            "Failed to find SHA256SUM for utreexod binary={} at path={}",
            bin_name, sha256sums_filename
        );
    }

    /// Download, verify, and extract the `utreexod` binary into
    /// `<CARGO_MANIFEST_DIR>/target/bin/utreexod-<VERSION>/utreexod`.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        const UTREEXOD_DOWNLOAD_URL: &str = "https://github.com/utreexo/utreexod/releases/download";

        let manifest_directory = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let download_directory = PathBuf::from(manifest_directory).join("target").join("bin");

        fs::create_dir_all(&download_directory)
            .map_err(|e| {
                format!(
                    "Cannot create download directory at={}: {:?}",
                    download_directory.display(),
                    e
                )
            })
            .unwrap();

        let existing_filename = download_directory
            .join(format!("utreexod-{}", UTREEXOD_VERSION))
            .join("utreexod");

        let download_filename = get_download_filename();
        let expected_hash = get_expected_sha256(&download_filename);

        if existing_filename.exists() {
            return;
        } else {
            println!(
                "cargo:warning=Downloading `utreexod` {} from `github.com`",
                download_filename
            );
        }

        let utreexod_tarball_bytes = {
            let download_url = format!(
                "{}/v{}/{}",
                UTREEXOD_DOWNLOAD_URL, UTREEXOD_VERSION, download_filename
            );

            let response = bitreq::get(&download_url)
                .send()
                .map_err(|e| format!("Failed to GET {}: {:?}", download_url, e))
                .unwrap();

            assert_eq!(
                response.status_code, 200,
                "Failed to GET {}: {} {}",
                download_url, response.status_code, response.reason_phrase
            );

            let utreexod_tarball = response.as_bytes().to_vec();

            utreexod_tarball
        };

        let utreexod_tarball_hash = sha256::Hash::hash(&utreexod_tarball_bytes);
        assert_eq!(
            utreexod_tarball_hash, expected_hash,
            "Downloaded utreexod binary hash does not match expected hash: downloaded={} != expected={}",
            utreexod_tarball_hash, expected_hash
        );

        let destination_directory =
            download_directory.join(format!("utreexod-{}", UTREEXOD_VERSION));
        fs::create_dir_all(&destination_directory)
            .map_err(|e| {
                format!(
                    "Cannot create destination directory={}: {}",
                    destination_directory.display(),
                    e
                )
            })
            .unwrap();

        let gz_decoder = GzDecoder::new(&utreexod_tarball_bytes[..]);
        let mut archive = Archive::new(gz_decoder);

        for mut entry in archive.entries().unwrap().flatten() {
            if let Ok(path) = entry.path() {
                if path.file_name() == Some(OsStr::new("utreexod")) {
                    let destination_path = destination_directory.join("utreexod");
                    let mut outputfile = File::create(&destination_path)
                        .map_err(|e| {
                            format!(
                                "Cannot create utreexod file at destination path={}: {}",
                                destination_path.display(),
                                e
                            )
                        })
                        .unwrap();

                    io::copy(&mut entry, &mut outputfile).unwrap();

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = outputfile.metadata().unwrap().permissions();
                        perms.set_mode(0o755);
                        outputfile.set_permissions(perms).unwrap();
                    }
                    break;
                }
            }
        }

        // MacOS (`arm64`) requires binaries to be code-signed locally for the OS to allow it's execution.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::process::Command;

            let signing_status = Command::new("codesign")
                .arg("-v")
                .arg(&existing_filename)
                .status()
                .map_err(|e| format!("Failed to run `codesign -v` on `utreexod`: {}", e))
                .unwrap();

            if !signing_status.success() {
                Command::new("codesign")
                    .arg("-s")
                    .arg("-")
                    .arg(&existing_filename)
                    .status()
                    .map_err(|e| format!("Failed to run `codesign -s` on `utreexod`: {}", e))
                    .unwrap();
            }
        }
    }
}
