// SPDX-License-Identifier: MIT OR Apache-2.0

//! Package Frigate 1.5.3 application images with their bundled Java runtimes.
//!
//! Run `just compile-bins package-frigate`. This requires 7-Zip's `7zz` executable
//! to unpack the macOS DMG inputs without mounting them.
//! Set `FRIGATE_7ZZ` to the executable path if it is not on `PATH`.
//! The resulting archives are placed in the repository's `frigate/` directory and must
//! be uploaded to both binary mirrors at `indexer/frigate/frigate-1.5.3/`.

#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Pinned Frigate release.
#[cfg(target_os = "macos")]
const VERSION: &str = "1.5.3";

/// Run a packaging command and require success.
#[cfg(target_os = "macos")]
fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().unwrap();
    assert!(status.success(), "{program} failed: {args:?}");
}

/// Compute a file's SHA-256 digest using the system checksum tool.
#[cfg(target_os = "macos")]
fn hash(path: &Path) -> String {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// Verify and repackage an upstream Linux app image.
#[cfg(target_os = "macos")]
fn package_linux(input: &Path, output: &Path, expected: &str, work: &Path) {
    assert_eq!(
        hash(input),
        expected,
        "upstream Linux archive checksum mismatch"
    );
    let stage = work.join("linux");
    if stage.exists() {
        fs::remove_dir_all(&stage).unwrap();
    }
    fs::create_dir_all(&stage).unwrap();
    run(
        "tar",
        &[
            "-xzf",
            input.to_str().unwrap(),
            "-C",
            stage.to_str().unwrap(),
        ],
    );
    assert!(stage.join("frigate/bin/frigate").is_file());
    run(
        "tar",
        &[
            "-czf",
            output.to_str().unwrap(),
            "-C",
            stage.to_str().unwrap(),
            "frigate",
        ],
    );
}

/// Verify and repackage an upstream macOS disk image.
#[cfg(target_os = "macos")]
fn package_macos(input: &Path, output: &Path, expected: &str, work: &Path) {
    assert_eq!(
        hash(input),
        expected,
        "upstream macOS image checksum mismatch"
    );
    let stage = work.join("macos");
    if stage.exists() {
        fs::remove_dir_all(&stage).unwrap();
    }
    fs::create_dir_all(&stage).unwrap();
    let sevenzip = std::env::var("FRIGATE_7ZZ").unwrap_or_else(|_| "7zz".to_string());
    let status = Command::new(&sevenzip)
        .args([
            "x",
            "-y",
            &format!("-o{}", stage.display()),
            input.to_str().unwrap(),
            "Frigate/Frigate.app/*",
        ])
        .status()
        .unwrap();
    // 7-Zip reports exit 2 for Java's relative legal-document symlinks.
    assert!(
        matches!(status.code(), Some(0 | 2)),
        "7-Zip failed: {status}"
    );
    let legal = stage.join("Frigate/Frigate.app/Contents/runtime/Contents/Home/legal");
    for module in [
        "java.datatransfer",
        "java.desktop",
        "java.logging",
        "java.naming",
        "java.prefs",
        "java.security.sasl",
        "java.sql",
        "java.transaction.xa",
        "java.xml",
    ] {
        for file in ["ADDITIONAL_LICENSE_INFO", "ASSEMBLY_EXCEPTION", "LICENSE"] {
            let link = legal.join(module).join(file);
            if !link.exists() {
                std::os::unix::fs::symlink(format!("../java.base/{file}"), link).unwrap();
            }
        }
    }
    fs::rename(stage.join("Frigate/Frigate.app"), stage.join("Frigate.app")).unwrap();
    let app = stage.join("Frigate.app");
    run(
        "find",
        &[
            app.to_str().unwrap(),
            "-name",
            "._*",
            "-type",
            "f",
            "-delete",
        ],
    );
    run(
        "codesign",
        &["--force", "--deep", "-s", "-", app.to_str().unwrap()],
    );
    run("codesign", &["-v", app.to_str().unwrap()]);
    assert!(stage.join("Frigate.app/Contents/MacOS/Frigate").is_file());
    run(
        "tar",
        &[
            "-czf",
            output.to_str().unwrap(),
            "-C",
            stage.to_str().unwrap(),
            "Frigate.app",
        ],
    );
}

#[cfg(target_os = "macos")]
fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/bins/package_frigate");
    let work = root.join("tmp");
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("frigate")
        .join(format!("frigate-{VERSION}"));
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&dist).unwrap();
    let artifacts = [
        (
            "Frigate-1.5.3-aarch64.dmg",
            "frigate-darwin-arm64.tar.gz",
            "045ffd0895fcfa616b1f462dae0f4ce089600472191812c824eeb464646c3b13",
            true,
        ),
        (
            "Frigate-1.5.3-x86_64.dmg",
            "frigate-darwin-amd64.tar.gz",
            "f27f3d7223ee8c64381fa60c0d854b913b43746c68ea5732d0b5d08fc5e41bec",
            true,
        ),
        (
            "frigate-1.5.3-aarch64.tar.gz",
            "frigate-linux-arm64.tar.gz",
            "28dd875dabab63b77876a96c15039fdca89533450fb990c67cbe0fbcbe340196",
            false,
        ),
        (
            "frigate-1.5.3-x86_64.tar.gz",
            "frigate-linux-amd64.tar.gz",
            "2f20f7640d7c596623aca6cd3a7ef0990c80c84b28f14eb921774a7c4dc7cdac",
            false,
        ),
    ];
    let mut sums = String::new();
    for (source, name, expected, macos) in artifacts {
        let input = work.join(source);
        if !input.exists() {
            let url = format!(
                "https://github.com/sparrowwallet/frigate/releases/download/{VERSION}/{source}"
            );
            run(
                "curl",
                &["-fL", "--retry", "3", "-o", input.to_str().unwrap(), &url],
            );
        }
        let output = dist.join(name);
        if macos {
            package_macos(&input, &output, expected, &work);
        } else {
            package_linux(&input, &output, expected, &work);
        }
        sums.push_str(&format!("{}  {name}\n", hash(&output)));
    }
    fs::write(dist.join("frigate-1.5.3-SHA256SUMS"), sums).unwrap();
    println!("Frigate archives: {}", dist.display());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Frigate packaging requires macOS");
    std::process::exit(1);
}
