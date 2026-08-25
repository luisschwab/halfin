// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build local release archives for `romanz/electrs`.
//!
//! Run this Cargo example from the repository root:
//!
//! ```text
//! cargo run --example cross-compile-electrs
//! ```
//!
//! The program gets the specified upstream release and builds each supported target.
//! It puts each program file in a separate archive.
//! It also writes the `SHA256SUMS` file for publication.

// Keep the builder pinned to the same
// `romanz/electrs` release that the crate downloads.
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/indexer/electrsd/versions.rs"
));

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use xshell::Shell;
use xshell::cmd;

/// `romanz/electrs` repository used as the release source.
const ELECTRS_REPO: &str = "https://github.com/romanz/electrs";

/// Build backend used for a target triple.
///
/// Native macOS targets use Cargo.
/// Linux targets use `cross`, and Windows MSVC targets use `cargo-xwin`.
#[derive(Debug, Clone, Copy)]
enum Builder {
    /// Use plain `cargo build` for targets the host toolchain can build directly.
    Cargo,
    /// Use `cross` for Linux targets that need containerized C dependencies.
    Cross,
    /// Use `cargo xwin` for Windows MSVC targets.
    CargoXwin,
}

/// Metadata needed to build and package one release artifact.
#[derive(Debug, Clone, Copy)]
struct Target {
    /// Rust target triple passed to Cargo.
    triple: &'static str,
    /// Published archive file name that `build.rs` expects.
    artifact_name: &'static str,
    /// Binary name inside the target release directory and final archive.
    exe_name: &'static str,
    /// Build backend for this target.
    builder: Builder,
    /// Extra clang target forwarded to bindgen for cross Linux builds.
    bindgen_args: Option<&'static str>,
}

/// All binaries published for a single `romanz/electrs` release.
const TARGETS: &[Target] = &[
    Target {
        triple: "aarch64-apple-darwin",
        artifact_name: "electrs-darwin-arm64.tar.gz",
        exe_name: "electrs",
        builder: Builder::Cargo,
        bindgen_args: None,
    },
    Target {
        triple: "x86_64-apple-darwin",
        artifact_name: "electrs-darwin-amd64.tar.gz",
        exe_name: "electrs",
        builder: Builder::Cargo,
        bindgen_args: None,
    },
    Target {
        triple: "x86_64-unknown-linux-gnu",
        artifact_name: "electrs-linux-amd64.tar.gz",
        exe_name: "electrs",
        builder: Builder::Cross,
        bindgen_args: Some("--target=x86_64-unknown-linux-gnu"),
    },
    Target {
        triple: "aarch64-unknown-linux-gnu",
        artifact_name: "electrs-linux-arm64.tar.gz",
        exe_name: "electrs",
        builder: Builder::Cross,
        bindgen_args: Some("--target=aarch64-unknown-linux-gnu"),
    },
    Target {
        triple: "x86_64-pc-windows-msvc",
        artifact_name: "electrs-windows-amd64.zip",
        exe_name: "electrs.exe",
        builder: Builder::CargoXwin,
        bindgen_args: None,
    },
    Target {
        triple: "aarch64-pc-windows-msvc",
        artifact_name: "electrs-windows-arm64.zip",
        exe_name: "electrs.exe",
        builder: Builder::CargoXwin,
        bindgen_args: None,
    },
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let force = parse_args()?;
    let sh = Shell::new()?;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compile_electrs_dir = manifest_dir.join("contrib/compile_electrs");
    let cross_config = compile_electrs_dir.join("Cross.toml");
    let dist_dir = compile_electrs_dir
        .join("dist")
        .join(format!("electrs-{ELECTRS_VERSION}"));
    let workdir = compile_electrs_dir.join("tmp");
    let source_dir = workdir.join("electrs");

    // Fail up front with actionable messages before spending time cloning or
    // compiling the `romanz/electrs` checkout.
    log_step("checking required tools");
    require_tools(&["git", "cargo", "rustup", "tar", "zip"])?;
    require_any_tool(&["shasum", "sha256sum"])?;
    require_cargo_subcommand("xwin")?;
    require_tool("cross")?;
    let container_engine = require_container_engine()?;
    log_step(format!(
        "using {container_engine} for cross container builds"
    ));

    log_step(format!("creating work directory {}", workdir.display()));
    sh.create_dir(&workdir)?;
    log_step(format!(
        "creating artifact directory {}",
        dist_dir.display()
    ));
    sh.create_dir(&dist_dir)?;
    prepare_source(&sh, &source_dir)?;
    install_targets(&sh)?;

    for target in TARGETS {
        let artifact = dist_dir.join(target.artifact_name);
        if artifact.exists() && !force {
            log_target(
                target,
                format!(
                    "skipping existing artifact {}; pass -- --force to rebuild",
                    artifact.display()
                ),
            );
            continue;
        }

        // Keep build and packaging separate so a failed archive verification
        // does not look like a compiler failure.
        log_target(target, "building");
        build_target(&sh, target, &source_dir, &cross_config, &container_engine)?;
        log_target(target, "packaging");
        package_target(&sh, target, &source_dir, &dist_dir)?;
    }

    log_step("writing SHA256SUMS");
    write_sha256sums(&dist_dir)?;

    println!("romanz/electrs {} artifacts:", ELECTRS_VERSION);
    for target in TARGETS {
        println!("  {}", dist_dir.join(target.artifact_name).display());
    }
    println!(
        "  {}",
        dist_dir
            .join(format!("electrs-{}-SHA256SUMS", ELECTRS_VERSION))
            .display()
    );

    Ok(())
}

/// Clone or reuse the upstream repository and reset it to the specified tag.
fn prepare_source(sh: &Shell, source_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source_dir_s = path_string(source_dir);

    if source_dir.exists() {
        log_step(format!(
            "using existing romanz/electrs checkout {}",
            source_dir.display()
        ));
    } else {
        log_step(format!(
            "cloning romanz/electrs {} into {}",
            ELECTRS_VERSION,
            source_dir.display()
        ));
        let repo = ELECTRS_REPO;
        cmd!(sh, "git clone {repo} {source_dir_s}").run_echo()?;
    }

    let sh = sh.with_current_dir(source_dir);
    log_step("fetching romanz/electrs tags");
    cmd!(sh, "git fetch --tags --force").run_echo()?;

    let tag = format!("v{ELECTRS_VERSION}");
    log_step(format!("checking out romanz/electrs {}", tag));
    cmd!(sh, "git checkout --force {tag}").run_echo()?;

    Ok(())
}

/// Return whether the build must replace existing artifacts.
fn parse_args() -> Result<bool, Box<dyn std::error::Error>> {
    let mut force = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            "-h" | "--help" => {
                println!(
                    "usage: cargo run --example cross-compile-electrs -- [--force]\n\n  --force    rebuild and repackage targets even when artifacts already exist"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument `{arg}`").into()),
        }
    }

    Ok(force)
}

/// Install all Rust target triples that the published artifacts require.
fn install_targets(sh: &Shell) -> Result<(), Box<dyn std::error::Error>> {
    for target in TARGETS {
        let triple = target.triple;
        log_target(target, "ensuring Rust target is installed");
        cmd!(sh, "rustup target add {triple}").run_echo()?;
    }
    Ok(())
}

/// Build one target with its configured backend.
fn build_target(
    sh: &Shell,
    target: &Target,
    source_dir: &Path,
    cross_config: &Path,
    container_engine: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let sh = sh.with_current_dir(source_dir);
    let triple = target.triple;

    match target.builder {
        Builder::Cargo => {
            log_target(target, "running cargo build");
            cmd!(sh, "cargo build --locked --release --target {triple}").run_echo()?;
        }
        Builder::Cross => {
            // bindgen/clang-sys can leave host-built artifacts under
            // target/release that confuse later cross builds. Remove only those
            // known build products before invoking `cross`.
            clean_stale_cross_bindgen_artifacts(source_dir)?;
            let cross_config_s = path_string(cross_config);
            log_target(
                target,
                format!("running cross build with {}", cross_config.display()),
            );
            let mut command = cmd!(sh, "cross build --locked --release --target {triple}");
            command = command.env("CROSS_CONFIG", cross_config_s);
            command = command.env("CROSS_CONTAINER_ENGINE", container_engine);
            command = command.env("DOCKER_DEFAULT_PLATFORM", "linux/amd64");
            // `romanz/electrs`' RocksDB bindings need a newer libclang than what some
            // cross base images expose by default.
            command = command.env("LIBCLANG_PATH", "/usr/lib/llvm-10/lib");
            command = command.env("CLANG_PATH", "/usr/bin/clang-10");
            if let Some(bindgen_args) = target.bindgen_args {
                log_target(
                    target,
                    format!("using BINDGEN_EXTRA_CLANG_ARGS={bindgen_args}"),
                );
                command = command.env("BINDGEN_EXTRA_CLANG_ARGS", bindgen_args);
            }
            command.run_echo()?;
        }
        Builder::CargoXwin => {
            log_target(target, "running cargo xwin build");
            cmd!(sh, "cargo xwin build --locked --release --target {triple}").run_echo()?;
        }
    }

    let binary = source_dir
        .join("target")
        .join(target.triple)
        .join("release")
        .join(target.exe_name);
    if !binary.is_file() {
        return Err(format!("expected built binary at {}", binary.display()).into());
    }
    log_target(target, format!("built {}", binary.display()));

    if host_can_run(target.triple) {
        // Prefer an executable sanity check for native builds.
        log_target(target, "running native version check");
        let version = output(Command::new(&binary).arg("--version"))?;
        println!("{}: {}", target.triple, version.trim());
    } else if let Some(file_output) = optional_output(Command::new("file").arg(&binary))? {
        // Cross-built binaries cannot be executed here, but `file` still gives
        // useful confirmation that the architecture and format look right.
        println!("{}", file_output.trim());
    }

    Ok(())
}

/// Package a built target binary into its published archive.
fn package_target(
    sh: &Shell,
    target: &Target,
    source_dir: &Path,
    dist_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let release_dir = source_dir
        .join("target")
        .join(target.triple)
        .join("release");
    let artifact = dist_dir.join(target.artifact_name);

    let _ = fs::remove_file(&artifact);
    log_target(target, format!("writing {}", artifact.display()));

    let release_dir_s = path_string(&release_dir);
    let artifact_s = path_string(&artifact);
    let exe_name = target.exe_name;

    if target.artifact_name.ends_with(".tar.gz") {
        cmd!(sh, "tar -czf {artifact_s} -C {release_dir_s} {exe_name}").run()?;
    } else {
        let binary = release_dir.join(target.exe_name);
        let binary_s = path_string(&binary);
        cmd!(sh, "zip -j -q {artifact_s} {binary_s}").run()?;
    }

    verify_archive(&artifact, target.exe_name)?;
    log_target(target, "archive verified");

    Ok(())
}

/// Remove old host `bindgen` artifacts that can cause subsequent cross builds to fail.
fn clean_stale_cross_bindgen_artifacts(
    source_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_release_dir = source_dir.join("target/release");
    remove_matching_entries(
        &target_release_dir.join("build"),
        &["clang-sys-", "bindgen-"],
    )?;
    remove_matching_entries(
        &target_release_dir.join(".fingerprint"),
        &["clang-sys-", "bindgen-"],
    )?;
    remove_matching_entries(
        &target_release_dir.join("deps"),
        &["clang_sys-", "libclang_sys-", "bindgen-", "libbindgen-"],
    )?;
    Ok(())
}

/// Remove known bindgen/clang-sys build outputs from a target subdirectory.
fn remove_matching_entries(
    dir: &Path,
    prefixes: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }

        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }

    Ok(())
}

/// Write the release `SHA256SUMS` file for all packaged artifacts.
fn write_sha256sums(dist_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut lines = Vec::new();
    for target in TARGETS {
        let artifact = dist_dir.join(target.artifact_name);
        let hash = sha256_file(&artifact)?;
        lines.push(format!("{}  {}", hash, target.artifact_name));
    }

    fs::write(
        dist_dir.join(format!("electrs-{}-SHA256SUMS", ELECTRS_VERSION)),
        format!("{}\n", lines.join("\n")),
    )?;

    Ok(())
}

/// Make sure that each published archive contains only the `romanz/electrs` binary.
fn verify_archive(artifact: &Path, exe_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let entries = if artifact.extension() == Some(OsStr::new("zip")) {
        output(Command::new("zip").arg("-sf").arg(artifact))?
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        output(Command::new("tar").arg("-tzf").arg(artifact))?
            .lines()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    };

    if entries.len() != 1 || entries[0] != exe_name {
        return Err(format!(
            "{} should contain only {}, got {:?}",
            artifact.display(),
            exe_name,
            entries
        )
        .into());
    }

    Ok(())
}

/// Hash an archive with an available local SHA256 tool.
fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if has_tool("shasum") {
        let out = output(Command::new("shasum").arg("-a").arg("256").arg(path))?;
        return out
            .split_whitespace()
            .next()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("failed to parse shasum output for {}", path.display()).into());
    }

    let out = output(Command::new("sha256sum").arg(path))?;
    out.split_whitespace()
        .next()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("failed to parse sha256sum output for {}", path.display()).into())
}

/// Require every named command-line tool to be available on `PATH`.
fn require_tools(tools: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    for tool in tools {
        require_tool(tool)?;
    }
    Ok(())
}

/// Require one command-line tool to be available on `PATH`.
fn require_tool(tool: &str) -> Result<(), Box<dyn std::error::Error>> {
    if has_tool(tool) {
        Ok(())
    } else {
        Err(format!("required tool `{}` was not found on PATH", tool).into())
    }
}

/// Require at least one command-line tool from `tools` to be available on `PATH`.
fn require_any_tool(tools: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    if tools.iter().any(|tool| has_tool(tool)) {
        Ok(())
    } else {
        Err(format!(
            "one of these tools is required on PATH: {}",
            tools.join(", ")
        )
        .into())
    }
}

/// Return the available container engine used by `cross`.
fn require_container_engine() -> Result<String, Box<dyn std::error::Error>> {
    if has_tool("docker") && command_succeeds(Command::new("docker").arg("info")) {
        return Ok("docker".to_string());
    }

    if has_tool("podman") && command_succeeds(Command::new("podman").arg("info")) {
        return Ok("podman".to_string());
    }

    Err(
        "no working container engine found; start Docker Desktop or Podman, then rerun the builder"
            .into(),
    )
}

/// Require a Cargo subcommand executable such as `cargo-xwin`.
fn require_cargo_subcommand(subcommand: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cargo_bin_name = format!("cargo-{}", subcommand);
    if has_tool(&cargo_bin_name) {
        Ok(())
    } else {
        Err(format!(
            "required cargo subcommand `{}` was not found on PATH",
            cargo_bin_name
        )
        .into())
    }
}

/// Return whether `PATH` contains `tool`.
fn has_tool(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg("command -v \"$1\" >/dev/null 2>&1")
        .arg("sh")
        .arg(tool)
        .status()
        .is_ok_and(|status| status.success())
}

/// Run `command` without output and return whether it succeeds.
fn command_succeeds(command: &mut Command) -> bool {
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Return whether the host can directly run binaries for `target`.
fn host_can_run(target: &str) -> bool {
    target == host_target_triple()
}

/// Return the Rust target triple for the current host when known.
fn host_target_triple() -> &'static str {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => "unknown",
    }
}

/// Run `command` and return UTF-8 `stdout` if it succeeds.
fn output(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "command failed with status {}: stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

/// Run `command` and return standard output.
/// Return `None` if the command is missing or fails.
fn optional_output(command: &mut Command) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match command.output() {
        Ok(output) if output.status.success() => Ok(Some(String::from_utf8(output.stdout)?)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Convert a path to an owned, lossy string for shell command interpolation.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Print a high-level build step.
fn log_step(message: impl AsRef<str>) {
    eprintln!("==> {}", message.as_ref());
}

/// Print a target-scoped build step.
fn log_target(target: &Target, message: impl AsRef<str>) {
    eprintln!("==> [{}] {}", target.triple, message.as_ref());
}
