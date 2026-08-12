// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build script for fetching and exposing binaries.
//!
//! The script downloads `bitcoind`, `florestad`, `utreexod`, `electrs`, and `electrumx`
//! archives when their features are enabled, verifies their checksums, extracts the needed
//! binaries, and publishes their paths through Cargo compile-time environment variables.

#[cfg(any(
    feature = "bitcoind",
    feature = "florestad",
    feature = "utreexod",
    feature = "electrs",
    feature = "electrumx"
))]
/// Shared binary download and extraction helpers.
mod binary {
    use std::collections::hash_map::RandomState;
    use std::env;
    use std::ffi::OsStr;
    use std::fs;
    use std::fs::File;
    use std::hash::BuildHasher;
    use std::io;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Cursor;
    use std::path::Path;
    pub(crate) use std::path::PathBuf;
    use std::str::FromStr;
    use std::time::Duration;

    use bitcoin_hashes::sha256;
    use bitreq::Method;
    use bitreq::Request;
    use bitreq::Url;
    use flate2::read::GzDecoder;
    use tar::Archive;

    /// Per-request timeout, in seconds, for binary archive downloads.
    const BIN_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

    /// Base URLs tried when downloading cached binary archives.
    const BIN_DOWNLOAD_MIRRORS: &[&str] =
        &["https://bin.luisschwab.net", "https://bin.lab.vinteum.org"];

    /// Return the root directory used to cache extracted binaries.
    fn download_directory() -> PathBuf {
        if let Ok(path) = env::var("HALFIN_BIN_DIR") {
            PathBuf::from(path)
        } else {
            PathBuf::from(env::var("OUT_DIR").unwrap()).join("bin")
        }
    }

    /// Mark extracted Unix executables as runnable.
    #[cfg(unix)]
    fn set_executable(file: &File) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = file.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        file.set_permissions(perms).unwrap();
    }

    /// Build-time metadata needed to fetch, verify, cache, and expose a binary.
    pub(crate) struct Binary {
        /// Name of the executable inside the downloaded archive.
        pub(crate) name: &'static str,

        /// Version displayed in build warnings and used in destination paths.
        pub(crate) version: &'static str,

        /// Compile-time environment variable that exposes the extracted binary path.
        pub(crate) env_var: &'static str,

        /// Prefix for the versioned directory that stores this binary.
        pub(crate) destination_dir_prefix: &'static str,

        /// Bundled SHA256SUMS file for this binary's archives.
        pub(crate) checksum_file: PathBuf,

        /// Top-level remote directory for this binary on the mirror.
        pub(crate) remote_dir: &'static str,

        /// Version-specific remote directory for this binary on the mirror.
        pub(crate) remote_version_dir: PathBuf,

        /// Platform-specific archive selected by the binary module.
        pub(crate) archive_filename: PathBuf,

        /// Whether macOS aarch64 builds should ad-hoc sign the extracted binary.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        pub(crate) codesign_on_macos_aarch64: bool,
    }

    impl Binary {
        /// Return the versioned directory name used for this binary.
        fn destination_dir_name(&self) -> String {
            format!("{}-{}", self.destination_dir_prefix, self.version)
        }

        /// Return the non-Windows path emitted as this binary's `HALFIN_*_PATH`.
        fn destination_path(&self, download_directory: &Path) -> PathBuf {
            download_directory
                .join(self.destination_dir_name())
                .join(self.name)
        }

        /// Return the executable path to check for an existing cached binary.
        fn existing_path(&self, download_directory: &Path) -> PathBuf {
            let path = self.destination_path(download_directory);

            if cfg!(windows) {
                path.with_extension("exe")
            } else {
                path
            }
        }

        /// Return the URL for this binary's selected platform archive on the selected mirror.
        fn download_url(&self, base_url: &str, archive_filename: &str) -> Url {
            Url::parse(&format!(
                "{}/{}/{}/{}",
                base_url,
                self.remote_dir,
                self.remote_version_dir.display(),
                archive_filename
            ))
            .unwrap()
        }

        /// Randomly select a mirror from [`BIN_DOWNLOAD_MIRRORS`].
        fn random_download_base_url_index(&self) -> usize {
            RandomState::new().hash_one((
                self.name,
                self.version,
                self.remote_dir,
                &self.remote_version_dir,
                &self.archive_filename,
            )) as usize
                % BIN_DOWNLOAD_MIRRORS.len()
        }

        /// Download, verify, and extract this binary.
        ///
        /// The binary is extracted into `<OUT_DIR>/bin/<destination-dir-prefix>-<VERSION>/<name>`,
        /// or `<HALFIN_BIN_DIR>/<destination-dir-prefix>-<VERSION>/<name>` if the
        /// `HALFIN_BIN_DIR` environment variable is set.
        ///
        /// Download is skipped if the binary is already cached from the expected archive.
        pub(crate) fn download_and_install(self) {
            println!("cargo:rerun-if-changed={}", self.checksum_file.display());

            let download_directory = download_directory();
            fs::create_dir_all(&download_directory)
                .map_err(|e| {
                    format!(
                        "Cannot create `{}` download directory at={}: {:?}",
                        self.name,
                        download_directory.display(),
                        e
                    )
                })
                .unwrap();

            let destination_path = self.destination_path(&download_directory);

            // Emit the binary path as an environment variable so runtime helpers can pick it up.
            println!(
                "cargo:rustc-env={}={}",
                self.env_var,
                destination_path.display()
            );

            let expected_hash = self.expected_sha256();

            let existing_path = self.existing_path(&download_directory);
            if existing_path.exists() {
                if self.cached_archive_hash_matches(&download_directory, expected_hash) {
                    println!(
                        "cargo:warning=Found cached `{}` @ v{} at `{}`, skipping download...",
                        self.name,
                        self.version,
                        existing_path.display(),
                    );
                    return;
                }

                println!(
                    "cargo:warning=Cached `{}` @ v{} at `{}` is stale, re-downloading...",
                    self.name,
                    self.version,
                    existing_path.display(),
                );
            }

            let archive_bytes = self.download_archive();
            self.install_archive(&archive_bytes, &download_directory, expected_hash);
        }

        /// Return the directory that stores this binary and its cache metadata.
        fn destination_directory(&self, download_directory: &Path) -> PathBuf {
            download_directory.join(self.destination_dir_name())
        }

        /// Return the sidecar file recording the archive hash used for the extracted binary.
        fn archive_hash_marker_path(&self, download_directory: &Path) -> PathBuf {
            self.destination_directory(download_directory)
                .join(".archive.sha256")
        }

        /// Return whether the cached binary was extracted from the expected archive hash.
        fn cached_archive_hash_matches(
            &self,
            download_directory: &Path,
            expected_hash: sha256::Hash,
        ) -> bool {
            let marker_path = self.archive_hash_marker_path(download_directory);
            fs::read_to_string(&marker_path)
                .is_ok_and(|hash| hash.trim() == expected_hash.to_string())
        }

        /// Look up the expected SHA256 hash for this binary's archive.
        ///
        /// Panics if the filename is not found in the checksum file.
        #[allow(clippy::lines_filter_map_ok)]
        fn expected_sha256(&self) -> sha256::Hash {
            let file = File::open(&self.checksum_file)
                .map_err(|e| {
                    format!(
                        "Cannot open `{}` SHA256SUMS file={}: {:?}",
                        self.name,
                        self.checksum_file.display(),
                        e
                    )
                })
                .unwrap();

            let archive_filename = self.archive_filename.to_string_lossy();
            for line in BufReader::new(file).lines().flatten() {
                let tokens: Vec<_> = line.split("  ").collect();
                if tokens.len() == 2 && archive_filename == tokens[1] {
                    return sha256::Hash::from_str(tokens[0]).unwrap();
                }
            }

            panic!(
                "Failed to find SHA256SUM for {} archive={} at path={}",
                self.name,
                self.archive_filename.display(),
                self.checksum_file.display()
            );
        }

        /// Download this binary's archive and return its raw bytes.
        fn download_archive(&self) -> Vec<u8> {
            let mut last_error = None;
            let start = self.random_download_base_url_index();
            let archive_filename = self.archive_filename.to_string_lossy();

            for offset in 0..BIN_DOWNLOAD_MIRRORS.len() {
                let base_url = BIN_DOWNLOAD_MIRRORS[(start + offset) % BIN_DOWNLOAD_MIRRORS.len()];
                let download_url: Url = self.download_url(base_url, &archive_filename);

                println!(
                    "cargo:warning=Downloading `{}` @ v{} from `{}`",
                    self.name, self.version, download_url,
                );

                let response = Request::new(Method::Get, download_url.as_str())
                    .with_timeout(BIN_DOWNLOAD_TIMEOUT.as_secs())
                    .send();

                match response {
                    Ok(response) if response.status_code == 200 => {
                        return response.as_bytes().to_vec();
                    }
                    Ok(response) => {
                        last_error = Some(format!(
                            "Failed to GET `{}`: {} {}",
                            download_url.as_str(),
                            response.status_code,
                            response.reason_phrase
                        ));
                    }
                    Err(err) => {
                        last_error = Some(format!("Failed to GET `{}`: {:?}", download_url, err));
                    }
                }
            }

            panic!(
                "Failed to download `{}` from all mirrors={:?}: {}",
                self.name,
                BIN_DOWNLOAD_MIRRORS,
                last_error.unwrap_or_else(|| "unknown error".to_string())
            );
        }

        /// Verify downloaded archive bytes against the expected SHA256 hash.
        fn verify_archive_hash(&self, archive_bytes: &[u8], expected_hash: sha256::Hash) {
            let downloaded_hash = sha256::Hash::hash(archive_bytes);
            assert_eq!(
                downloaded_hash, expected_hash,
                "Downloaded {} archive hash does not match expected hash: downloaded={} != expected={}",
                self.name, downloaded_hash, expected_hash
            );
        }

        /// Verify and extract archive bytes into the cache directory.
        fn install_archive(
            &self,
            archive_bytes: &[u8],
            download_directory: &Path,
            expected_hash: sha256::Hash,
        ) {
            self.verify_archive_hash(archive_bytes, expected_hash);
            self.extract_archive(archive_bytes, download_directory);

            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if self.codesign_on_macos_aarch64 {
                let destination_path = self.destination_path(download_directory);
                self.codesign_on_macos_aarch64(&destination_path);
            }

            let marker_path = self.archive_hash_marker_path(download_directory);
            fs::write(&marker_path, format!("{expected_hash}\n"))
                .map_err(|e| {
                    format!(
                        "Cannot write `{}` archive hash marker at={}: {}",
                        self.name,
                        marker_path.display(),
                        e
                    )
                })
                .unwrap();
        }

        /// Extract the selected archive format into this binary's destination directory.
        fn extract_archive(&self, archive_bytes: &[u8], download_directory: &Path) {
            let destination_directory = self.destination_directory(download_directory);
            if destination_directory.exists() {
                fs::remove_dir_all(&destination_directory)
                    .map_err(|e| {
                        format!(
                            "Cannot remove stale destination directory={}: {}",
                            destination_directory.display(),
                            e
                        )
                    })
                    .unwrap();
            }
            fs::create_dir_all(&destination_directory)
                .map_err(|e| {
                    format!(
                        "Cannot create destination directory={}: {}",
                        destination_directory.display(),
                        e
                    )
                })
                .unwrap();

            let archive_filename = self.archive_filename.to_string_lossy();
            if archive_filename.ends_with(".tar.gz") {
                self.extract_tar_gz(archive_bytes, &destination_directory);
            } else if archive_filename.ends_with(".zip") {
                self.extract_zip(archive_bytes, &destination_directory);
            } else {
                panic!(
                    "Unsupported archive format: {}",
                    self.archive_filename.display()
                );
            }
        }

        /// Extract a Unix-style executable from a `.tar.gz` archive.
        fn extract_tar_gz(&self, archive_bytes: &[u8], destination_directory: &Path) {
            let gz_decoder = GzDecoder::new(archive_bytes);
            let mut archive = Archive::new(gz_decoder);

            for mut entry in archive.entries().unwrap().flatten() {
                if entry
                    .path()
                    .is_ok_and(|path| path.file_name() == Some(OsStr::new(self.name)))
                {
                    let destination_path = destination_directory.join(self.name);
                    let mut output_file = self.create_binary_file(&destination_path);
                    io::copy(&mut entry, &mut output_file).unwrap();
                    #[cfg(unix)]
                    set_executable(&output_file);
                    return;
                }
            }

            panic!("Failed to find `{}` in downloaded archive", self.name);
        }

        /// Extract a Windows executable from a `.zip` archive.
        fn extract_zip(&self, archive_bytes: &[u8], destination_directory: &Path) {
            let cursor = Cursor::new(archive_bytes);
            let mut archive = zip::ZipArchive::new(cursor).unwrap();
            let executable_name = format!("{}.exe", self.name);

            for i in 0..archive.len() {
                let mut file = archive.by_index(i).unwrap();
                if file
                    .enclosed_name()
                    .is_some_and(|p| p.file_name() == Some(OsStr::new(&executable_name)))
                {
                    let destination_path = destination_directory.join(&executable_name);
                    let mut output_file = self.create_binary_file(&destination_path);
                    io::copy(&mut file, &mut output_file).unwrap();
                    return;
                }
            }

            panic!("Failed to find `{executable_name}` in downloaded archive");
        }

        /// Create the destination executable file.
        fn create_binary_file(&self, destination_path: &Path) -> File {
            File::create(destination_path)
                .map_err(|e| {
                    format!(
                        "Cannot create `{}` at destination={}: {}",
                        self.name,
                        destination_path.display(),
                        e
                    )
                })
                .unwrap()
        }

        /// Sign macOS aarch64 binaries ad hoc if they are not already accepted by `codesign -v`.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        fn codesign_on_macos_aarch64(&self, destination_path: &Path) {
            use std::process::Command;

            let signing_status = Command::new("codesign")
                .arg("-v")
                .arg(destination_path)
                .status()
                .map_err(|e| format!("Failed to run `codesign -v` on `{}`: {}", self.name, e))
                .unwrap();

            if !signing_status.success() {
                Command::new("codesign")
                    .arg("-s")
                    .arg("-")
                    .arg(destination_path)
                    .status()
                    .map_err(|e| format!("Failed to run `codesign -s` on `{}`: {}", self.name, e))
                    .unwrap();
            }
        }
    }
}

fn main() {
    use std::env;

    if env::var("DOCS_RS").is_err() {
        #[cfg(feature = "bitcoind")]
        bitcoind::download();

        #[cfg(feature = "florestad")]
        florestad::download();

        #[cfg(feature = "utreexod")]
        utreexod::download();

        #[cfg(feature = "electrs")]
        electrs::download();

        #[cfg(feature = "electrumx")]
        electrumx::download();
    }
}

/// Downloads and verifies the `bitcoind` binary based on the enabled version feature.
#[cfg(feature = "bitcoind")]
mod bitcoind {
    use super::binary::Binary;
    use super::binary::PathBuf;

    include!("src/node/bitcoind/versions.rs");

    /// Compile-time environment variable containing the extracted `bitcoind` path.
    const HALFIN_BITCOIND_PATH: &str = "HALFIN_BITCOIND_PATH";

    /// Return the platform-specific tarball filename for this version of `bitcoind`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return format!("bitcoin-{}-arm64-apple-darwin.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return format!("bitcoin-{}-x86_64-apple-darwin.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return format!("bitcoin-{}-x86_64-linux-gnu.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return format!("bitcoin-{}-aarch64-linux-gnu.tar.gz", BITCOIND_VERSION);
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return format!("bitcoin-{}-win64.zip", BITCOIND_VERSION);
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Download, verify, and extract the `bitcoind` binary into
    /// `<OUT_DIR>/bin/bitcoin-<VERSION>/bitcoind`, or
    /// `<HALFIN_BIN_DIR>/bitcoin-<VERSION>/bitcoind` if the
    /// `HALFIN_BIN_DIR` environment variable is set.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        Binary {
            name: "bitcoind",
            version: BITCOIND_VERSION,
            env_var: HALFIN_BITCOIND_PATH,
            destination_dir_prefix: "bitcoin",
            checksum_file: PathBuf::from(format!(
                "sha256/bitcoind/bitcoin-core-{}-SHA256SUMS",
                BITCOIND_VERSION
            )),
            remote_dir: "bitcoind",
            remote_version_dir: PathBuf::from(format!("bitcoin-core-{}", BITCOIND_VERSION)),
            archive_filename: PathBuf::from(get_download_filename()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            codesign_on_macos_aarch64: true,
        }
        .download_and_install();
    }
}

/// Downloads and verifies the `florestad` binary based on the enabled version feature.
#[cfg(feature = "florestad")]
mod florestad {
    use super::binary::Binary;
    use super::binary::PathBuf;

    include!("src/node/florestad/versions.rs");

    /// Compile-time environment variable containing the extracted `florestad` path.
    const HALFIN_FLORESTAD_PATH: &str = "HALFIN_FLORESTAD_PATH";

    /// Return the platform-specific archive filename for this version of `florestad`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return "florestad-darwin-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return "florestad-linux-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return "florestad-linux-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return "florestad-windows-amd64.zip".to_string();
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Download, verify, and extract the `florestad` binary into
    /// `<OUT_DIR>/bin/florestad-<VERSION>/florestad`, or
    /// `<HALFIN_BIN_DIR>/florestad-<VERSION>/florestad` if the
    /// `HALFIN_BIN_DIR` environment variable is set.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        Binary {
            name: "florestad",
            version: FLORESTAD_VERSION,
            env_var: HALFIN_FLORESTAD_PATH,
            destination_dir_prefix: "florestad",
            checksum_file: PathBuf::from(format!(
                "sha256/florestad/florestad-{}-SHA256SUMS",
                FLORESTAD_VERSION
            )),
            remote_dir: "florestad",
            remote_version_dir: PathBuf::from(format!("florestad-{}", FLORESTAD_VERSION)),
            archive_filename: PathBuf::from(get_download_filename()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            codesign_on_macos_aarch64: true,
        }
        .download_and_install();
    }
}

/// Downloads and verifies the `utreexod` binary based on the enabled version feature.
#[cfg(feature = "utreexod")]
mod utreexod {
    use super::binary::Binary;
    use super::binary::PathBuf;

    include!("src/node/utreexod/versions.rs");

    /// Compile-time environment variable containing the extracted `utreexod` path.
    const HALFIN_UTREEXOD_PATH: &str = "HALFIN_UTREEXOD_PATH";

    /// Return the platform-specific tarball filename for this version of `utreexod`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return "utreexod-darwin-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return "utreexod-darwin-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return "utreexod-linux-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return "utreexod-linux-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return "utreexod-windows-amd64.zip".to_string();
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Download, verify, and extract the `utreexod` binary into
    /// `<OUT_DIR>/bin/utreexod-<VERSION>/utreexod`, or
    /// `<HALFIN_BIN_DIR>/utreexod-<VERSION>/utreexod` if the
    /// `HALFIN_BIN_DIR` environment variable is set.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        Binary {
            name: "utreexod",
            version: UTREEXOD_VERSION,
            env_var: HALFIN_UTREEXOD_PATH,
            destination_dir_prefix: "utreexod",
            checksum_file: PathBuf::from(format!(
                "sha256/utreexod/utreexod-{}-SHA256SUMS",
                UTREEXOD_VERSION
            )),
            remote_dir: "utreexod",
            remote_version_dir: PathBuf::from(format!("utreexod-{}", UTREEXOD_VERSION)),
            archive_filename: PathBuf::from(get_download_filename()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            codesign_on_macos_aarch64: true,
        }
        .download_and_install();
    }
}

/// Downloads and verifies the `electrs` binary based on the enabled version feature.
#[cfg(feature = "electrs")]
mod electrs {
    use super::binary::Binary;
    use super::binary::PathBuf;

    include!("src/indexer/electrsd/versions.rs");

    /// Compile-time environment variable containing the extracted `electrs` path.
    const HALFIN_ELECTRS_PATH: &str = "HALFIN_ELECTRS_PATH";

    /// Return the platform-specific archive filename for this version of `electrs`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return "electrs-darwin-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return "electrs-darwin-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return "electrs-linux-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return "electrs-linux-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return "electrs-windows-amd64.zip".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            return "electrs-windows-arm64.zip".to_string();
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Read, verify, and extract the `electrs` binary into
    /// `<OUT_DIR>/bin/electrs-<VERSION>/electrs`, or
    /// `<HALFIN_BIN_DIR>/electrs-<VERSION>/electrs` if the
    /// `HALFIN_BIN_DIR` environment variable is set.
    ///
    /// Skips extraction if the binary is already cached from a previous build.
    pub(crate) fn download() {
        Binary {
            name: "electrs",
            version: ELECTRS_VERSION,
            env_var: HALFIN_ELECTRS_PATH,
            destination_dir_prefix: "electrs",
            checksum_file: PathBuf::from(format!(
                "sha256/electrs/electrs-{}-SHA256SUMS",
                ELECTRS_VERSION
            )),
            remote_dir: "electrs",
            remote_version_dir: PathBuf::from(format!("electrs-{}", ELECTRS_VERSION)),
            archive_filename: PathBuf::from(get_download_filename()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            codesign_on_macos_aarch64: false,
        }
        .download_and_install();
    }
}

/// Downloads and verifies the `ElectrumX` launcher based on the enabled version feature.
#[cfg(feature = "electrumx")]
mod electrumx {
    use super::binary::Binary;
    use super::binary::PathBuf;

    include!("src/indexer/electrumxd/versions.rs");

    /// Compile-time environment variable containing the extracted `ElectrumX` launcher path.
    const HALFIN_ELECTRUMX_PATH: &str = "HALFIN_ELECTRUMX_PATH";

    /// Return the platform-specific archive filename for this version of `ElectrumX`.
    ///
    /// Panics if the current OS/architecture combination is not supported.
    fn get_download_filename() -> String {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return "electrumx-darwin-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            return "electrumx-darwin-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return "electrumx-linux-amd64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return "electrumx-linux-arm64.tar.gz".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return "electrumx-windows-amd64.zip".to_string();
        }
        if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            return "electrumx-windows-arm64.zip".to_string();
        }
        panic!("No download file for this OS+Architecture combination");
    }

    /// Download, verify, and extract the `ElectrumX` launcher into
    /// `<OUT_DIR>/bin/electrumx-<VERSION>/electrumx`, or
    /// `<HALFIN_BIN_DIR>/electrumx-<VERSION>/electrumx` if the
    /// `HALFIN_BIN_DIR` environment variable is set.
    ///
    /// Skips the download if the binary is already cached from a previous build.
    pub(crate) fn download() {
        Binary {
            name: "electrumx",
            version: ELECTRUMX_VERSION,
            env_var: HALFIN_ELECTRUMX_PATH,
            destination_dir_prefix: "electrumx",
            checksum_file: PathBuf::from(format!(
                "sha256/electrumx/electrumx-{}-SHA256SUMS",
                ELECTRUMX_VERSION
            )),
            remote_dir: "electrumx",
            remote_version_dir: PathBuf::from(format!("electrumx-{}", ELECTRUMX_VERSION)),
            archive_filename: PathBuf::from(get_download_filename()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            codesign_on_macos_aarch64: false,
        }
        .download_and_install();
    }
}
