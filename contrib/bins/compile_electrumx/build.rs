// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build local release archives for the `ElectrumX` Python application.
//!
//! Run this Cargo example from the repository root:
//!
//! ```text
//! cargo run --example cross-compile-electrumx
//! ```
//!
//! The program gets the specified upstream release and builds its Python wheel.
//! It gets the dependency wheels for each platform and builds each launcher archive.
//! It also writes the `SHA256SUMS` file for publication.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use xshell::Shell;
use xshell::cmd;

/// Upstream `ElectrumX` repository used as the release source.
const ELECTRUMX_REPO: &str = "https://github.com/spesmilo/electrumx";

/// Upstream `ElectrumX` version packaged by this builder.
const ELECTRUMX_VERSION: &str = "1.20.0";

/// Magic prefix for the embedded wheelhouse archive that uses only the standard library.
const WHEELHOUSE_MAGIC: &[u8] = b"HALFIN_ELECTRUMX_WHEELHOUSE_V1\0";

/// Build backend used for the compiled `ElectrumX` launcher.
#[derive(Debug, Clone, Copy)]
enum Builder {
    /// Use plain `cargo build` for targets the host toolchain can build directly.
    Cargo,
    /// Use `cross` for Linux targets.
    Cross,
    /// Use `cargo xwin` for Windows MSVC targets.
    CargoXwin,
}

/// Metadata needed to build and package one release artifact.
#[derive(Debug, Clone, Copy)]
struct Target {
    /// Rust target triple passed to Cargo when compiling the launcher.
    triple: &'static str,
    /// Pip platform tag used when downloading binary wheels.
    platform: &'static str,
    /// Python version passed to pip when downloading target wheels.
    python_version: &'static str,
    /// Python ABI tag passed to pip when downloading target wheels.
    abi: &'static str,
    /// Published archive file name that the download code expects.
    artifact_name: &'static str,
    /// Compiled launcher name inside the final bundle.
    exe_name: &'static str,
    /// Build backend for the compiled launcher.
    builder: Builder,
    /// Identifies an artifact for Windows.
    windows: bool,
    /// Directory under `local_wheels/` containing locally built native wheels.
    local_wheel_dir: Option<&'static str>,
}

/// Python application bundles published for a single `ElectrumX` release.
const TARGETS: &[Target] = &[
    Target {
        triple: "aarch64-apple-darwin",
        platform: "macosx_11_0_arm64",
        python_version: "310",
        abi: "cp310",
        artifact_name: "electrumx-darwin-arm64.tar.gz",
        exe_name: "electrumx",
        builder: Builder::Cargo,
        windows: false,
        local_wheel_dir: Some("macosx_11_0_arm64"),
    },
    Target {
        triple: "x86_64-apple-darwin",
        platform: "macosx_10_9_x86_64",
        python_version: "310",
        abi: "cp310",
        artifact_name: "electrumx-darwin-amd64.tar.gz",
        exe_name: "electrumx",
        builder: Builder::Cargo,
        windows: false,
        local_wheel_dir: Some("macosx_10_9_x86_64"),
    },
    Target {
        triple: "x86_64-unknown-linux-gnu",
        platform: "manylinux_2_28_x86_64",
        python_version: "310",
        abi: "cp310",
        artifact_name: "electrumx-linux-amd64.tar.gz",
        exe_name: "electrumx",
        builder: Builder::Cross,
        windows: false,
        local_wheel_dir: Some("manylinux_2_28_x86_64"),
    },
    Target {
        triple: "aarch64-unknown-linux-gnu",
        platform: "manylinux_2_28_aarch64",
        python_version: "310",
        abi: "cp310",
        artifact_name: "electrumx-linux-arm64.tar.gz",
        exe_name: "electrumx",
        builder: Builder::Cross,
        windows: false,
        local_wheel_dir: Some("manylinux_2_28_aarch64"),
    },
    Target {
        triple: "x86_64-pc-windows-msvc",
        platform: "win_amd64",
        python_version: "310",
        abi: "cp310",
        artifact_name: "electrumx-windows-amd64.zip",
        exe_name: "electrumx.exe",
        builder: Builder::CargoXwin,
        windows: true,
        local_wheel_dir: Some("win_amd64"),
    },
    Target {
        triple: "aarch64-pc-windows-msvc",
        platform: "win_arm64",
        python_version: "311",
        abi: "cp311",
        artifact_name: "electrumx-windows-arm64.zip",
        exe_name: "electrumx.exe",
        builder: Builder::CargoXwin,
        windows: true,
        local_wheel_dir: Some("win_arm64"),
    },
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let force = parse_args()?;
    let sh = Shell::new()?;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compile_electrumx_dir = manifest_dir.join("contrib/bins/compile_electrumx");
    let launcher_dir = compile_electrumx_dir.join("launcher");
    let plyvel_cross_builder = compile_electrumx_dir.join("build_plyvel_windows.py");
    let local_wheels_dir = compile_electrumx_dir.join("local_wheels");
    let dist_dir = compile_electrumx_dir
        .join("dist")
        .join(format!("electrumx-{ELECTRUMX_VERSION}"));
    let workdir = compile_electrumx_dir.join("tmp");
    let source_dir = workdir.join("electrumx");
    let build_venv_dir = workdir.join("build-venv");
    let uv_cache_dir = workdir.join("uv-cache");
    let uv_python_install_dir = workdir.join("uv-python");
    let wheelhouse_dir = workdir.join("wheelhouse");
    let package_dir = workdir.join("packages");
    let plyvel_cross_dir = workdir.join("plyvel-cross");

    log_step("checking required tools");
    require_tools(&["git", "uv", "cargo", "rustup", "tar", "zip"])?;
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
    let build_python =
        prepare_python_build_env(&sh, &build_venv_dir, &uv_cache_dir, &uv_python_install_dir)?;
    build_electrumx_wheel(&sh, &build_python, &source_dir, &wheelhouse_dir)?;

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

        log_target(target, "building wheelhouse");
        let target_package_dir = package_dir.join(target.platform);
        build_target_bundle(
            &sh,
            target,
            &build_python,
            &wheelhouse_dir,
            &target_package_dir,
            &launcher_dir,
            &local_wheels_dir,
            &plyvel_cross_dir,
            &plyvel_cross_builder,
            &container_engine,
        )?;
        log_target(target, "packaging");
        package_target(&sh, target, &target_package_dir, &dist_dir)?;
    }

    log_step("writing SHA256SUMS");
    write_sha256sums(&dist_dir)?;

    println!("ElectrumX {} artifacts:", ELECTRUMX_VERSION);
    for target in TARGETS {
        println!("  {}", dist_dir.join(target.artifact_name).display());
    }
    println!(
        "  {}",
        dist_dir
            .join(format!("electrumx-{}-SHA256SUMS", ELECTRUMX_VERSION))
            .display()
    );

    Ok(())
}

/// Clone or reuse the upstream repository and reset it to the specified tag.
fn prepare_source(sh: &Shell, source_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source_dir_s = path_string(source_dir);

    if source_dir.exists() {
        log_step(format!(
            "using existing ElectrumX checkout {}",
            source_dir.display()
        ));
    } else {
        log_step(format!(
            "cloning ElectrumX {} into {}",
            ELECTRUMX_VERSION,
            source_dir.display()
        ));
        let repo = ELECTRUMX_REPO;
        cmd!(sh, "git clone {repo} {source_dir_s}").run_echo()?;
    }

    let sh = sh.with_current_dir(source_dir);
    log_step("fetching ElectrumX tags");
    cmd!(sh, "git fetch --tags --force").run_echo()?;

    let tag = ELECTRUMX_VERSION;
    log_step(format!("checking out ElectrumX {}", tag));
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
                    "usage: cargo run --example cross-compile-electrumx -- [--force]\n\n  --force    rebuild and repackage targets even when artifacts already exist"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument `{arg}`").into()),
        }
    }

    Ok(force)
}

/// Create the local `uv`-managed Python build environment.
fn prepare_python_build_env(
    sh: &Shell,
    build_venv_dir: &Path,
    uv_cache_dir: &Path,
    uv_python_install_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    log_step(format!(
        "creating uv build venv {}",
        build_venv_dir.display()
    ));
    let build_venv_dir_s = path_string(build_venv_dir);
    let uv_cache_dir_s = path_string(uv_cache_dir);
    let uv_python_install_dir_s = path_string(uv_python_install_dir);
    cmd!(
        sh,
        "uv venv --no-project --clear --seed --python 3.10 {build_venv_dir_s}"
    )
    .env("UV_CACHE_DIR", &uv_cache_dir_s)
    .env("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir_s)
    .run_echo()?;

    let python = venv_python(build_venv_dir);
    let python_s = path_string(&python);
    log_step("installing Python build tools into uv venv");
    cmd!(sh, "uv pip install --python {python_s} build")
        .env("UV_CACHE_DIR", &uv_cache_dir_s)
        .env("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir_s)
        .run_echo()?;

    run_python(&python, &["-m", "pip", "--version"])?;
    run_python(&python, &["-m", "build", "--version"])?;

    Ok(python)
}

/// Build the `ElectrumX` wheel once from the pinned checkout.
fn build_electrumx_wheel(
    sh: &Shell,
    build_python: &Path,
    source_dir: &Path,
    wheelhouse_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = fs::remove_dir_all(wheelhouse_dir);
    sh.create_dir(wheelhouse_dir)?;

    log_step("building ElectrumX wheel");
    let outdir = path_string(wheelhouse_dir);
    let source_dir_s = path_string(source_dir);
    let build_python_s = path_string(build_python);
    cmd!(
        sh,
        "{build_python_s} -m build --wheel --outdir {outdir} {source_dir_s}"
    )
    .run_echo()?;

    let expected_prefix = format!("e_x-{ELECTRUMX_VERSION}-");
    if !fs::read_dir(wheelhouse_dir)?.any(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .is_some_and(|name| {
                name.starts_with(&expected_prefix)
                    && Path::new(&name)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
            })
    }) {
        return Err(format!(
            "expected an ElectrumX wheel starting with {expected_prefix} in {}",
            wheelhouse_dir.display()
        )
        .into());
    }

    Ok(())
}

/// Install all Rust target triples that the published launchers require.
fn install_targets(sh: &Shell) -> Result<(), Box<dyn std::error::Error>> {
    for target in TARGETS {
        let triple = target.triple;
        log_target(target, "ensuring Rust target is installed");
        cmd!(sh, "rustup target add {triple}").run_echo()?;
    }
    Ok(())
}

/// Build one target bundle with an `ElectrumX` wheelhouse and compiled launcher.
#[allow(clippy::too_many_arguments)]
fn build_target_bundle(
    sh: &Shell,
    target: &Target,
    build_python: &Path,
    shared_wheelhouse_dir: &Path,
    target_package_dir: &Path,
    launcher_dir: &Path,
    local_wheels_dir: &Path,
    plyvel_cross_dir: &Path,
    plyvel_cross_builder: &Path,
    container_engine: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = fs::remove_dir_all(target_package_dir);
    sh.create_dir(target_package_dir)?;
    let wheelhouse_dir = target_package_dir.join("wheelhouse");
    sh.create_dir(&wheelhouse_dir)?;

    copy_wheels(shared_wheelhouse_dir, &wheelhouse_dir)?;
    download_dependency_wheels(
        target,
        build_python,
        &wheelhouse_dir,
        local_wheels_dir,
        plyvel_cross_dir,
        plyvel_cross_builder,
    )?;
    write_embedded_wheelhouse(launcher_dir, &wheelhouse_dir)?;
    build_launcher(sh, target, launcher_dir, container_engine)?;
    copy_launcher(target, launcher_dir, target_package_dir)?;

    Ok(())
}

/// Write the target wheelhouse into the launcher source tree for `include_bytes!`.
fn write_embedded_wheelhouse(
    launcher_dir: &Path,
    wheelhouse_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let embedded_path = launcher_dir.join("embedded_wheelhouse.bin");
    let mut wheels = fs::read_dir(wheelhouse_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    wheels.retain(|path| path.is_file());
    wheels.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    if wheels.is_empty() {
        return Err(format!("no files found in wheelhouse {}", wheelhouse_dir.display()).into());
    }

    let mut file = fs::File::create(&embedded_path)?;
    file.write_all(WHEELHOUSE_MAGIC)?;
    file.write_all(&(wheels.len() as u32).to_le_bytes())?;

    for wheel in wheels {
        let name = wheel
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| format!("invalid wheel path {}", wheel.display()))?;
        let contents = fs::read(&wheel)?;
        file.write_all(&(name.len() as u32).to_le_bytes())?;
        file.write_all(&(contents.len() as u64).to_le_bytes())?;
        file.write_all(name.as_bytes())?;
        file.write_all(&contents)?;
    }

    Ok(())
}

/// Build the small Rust executable that launches the bundled `ElectrumX` wheelhouse.
fn build_launcher(
    sh: &Shell,
    target: &Target,
    launcher_dir: &Path,
    container_engine: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = launcher_dir.join("Cargo.toml");
    let manifest_path_s = path_string(&manifest_path);
    let triple = target.triple;

    match target.builder {
        Builder::Cargo => {
            log_target(target, "running cargo build for launcher");
            cmd!(
                sh,
                "cargo build --locked --release --manifest-path {manifest_path_s} --target {triple}"
            )
            .run_echo()?;
        }
        Builder::Cross => {
            log_target(target, "running cross build for launcher");
            cmd!(
                sh,
                "cross build --locked --release --manifest-path {manifest_path_s} --target {triple}"
            )
            .env("CROSS_CONTAINER_ENGINE", container_engine)
            .env("DOCKER_DEFAULT_PLATFORM", "linux/amd64")
            .run_echo()?;
        }
        Builder::CargoXwin => {
            log_target(target, "running cargo xwin build for launcher");
            cmd!(
                sh,
                "cargo xwin build --locked --release --manifest-path {manifest_path_s} --target {triple}"
            )
            .run_echo()?;
        }
    }

    let binary = launcher_dir
        .join("target")
        .join(target.triple)
        .join("release")
        .join(target.exe_name);
    if !binary.is_file() {
        return Err(format!("expected built launcher at {}", binary.display()).into());
    }
    log_target(target, format!("built {}", binary.display()));

    Ok(())
}

/// Copy the compiled launcher binary into the package bundle.
fn copy_launcher(
    target: &Target,
    launcher_dir: &Path,
    bundle_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let from = launcher_dir
        .join("target")
        .join(target.triple)
        .join("release")
        .join(target.exe_name);
    let to = bundle_dir.join(target.exe_name);
    fs::copy(&from, &to)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if !target.windows {
            let mut permissions = fs::metadata(&to)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&to, permissions)?;
        }
    }

    Ok(())
}

/// Download binary wheels for dependencies for a single target platform.
fn download_dependency_wheels(
    target: &Target,
    build_python: &Path,
    wheelhouse_dir: &Path,
    local_wheels_dir: &Path,
    plyvel_cross_dir: &Path,
    plyvel_cross_builder: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let dest = path_string(wheelhouse_dir);
    let platform = target.platform;
    let requirements = ["aiorpcx[ws]>=0.25.0,<0.26", "aiohttp>=3.3,<4"];

    log_target(
        target,
        format!("downloading dependency wheels for pip platform {platform}"),
    );
    let mut args = vec![
        "-m",
        "pip",
        "download",
        "--dest",
        &dest,
        "--only-binary=:all:",
        "--implementation",
        "cp",
        "--python-version",
        target.python_version,
        "--abi",
        target.abi,
        "--platform",
        platform,
    ];
    args.extend(requirements);
    run_python(build_python, &args)?;
    if let Some(local_wheel_dir) = target.local_wheel_dir {
        ensure_local_plyvel_wheel(
            target,
            build_python,
            local_wheels_dir,
            local_wheel_dir,
            plyvel_cross_dir,
            plyvel_cross_builder,
        )?;
        copy_local_plyvel_wheel(target, local_wheels_dir, local_wheel_dir, wheelhouse_dir)?;
    }

    Ok(())
}

/// Ensure a locally built `plyvel` wheel exists for a platform target.
fn ensure_local_plyvel_wheel(
    target: &Target,
    build_python: &Path,
    local_wheels_dir: &Path,
    local_wheel_dir: &str,
    plyvel_cross_dir: &Path,
    plyvel_cross_builder: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_wheels_dir = local_wheels_dir.join(local_wheel_dir);
    if find_local_plyvel_wheels(&target_wheels_dir)?.len() == 1 {
        return Ok(());
    }

    sh_create_dir_all(&target_wheels_dir)?;
    build_local_plyvel_wheel(
        target,
        build_python,
        local_wheel_dir,
        &target_wheels_dir,
        plyvel_cross_dir,
        plyvel_cross_builder,
    )?;

    match find_local_plyvel_wheels(&target_wheels_dir)?.len() {
        1 => Ok(()),
        count => Err(format!(
            "expected the local plyvel build for {} to produce exactly one wheel, found {} under {}",
            target.platform,
            count,
            target_wheels_dir.display()
        )
        .into()),
    }
}

/// Copy the locally built `plyvel` wheel required by a platform target.
fn copy_local_plyvel_wheel(
    target: &Target,
    local_wheels_dir: &Path,
    local_wheel_dir: &str,
    wheelhouse_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_wheels_dir = local_wheels_dir.join(local_wheel_dir);
    let matches = find_local_plyvel_wheels(&target_wheels_dir)?;

    match matches.as_slice() {
        [wheel] => {
            log_target(
                target,
                format!("using local plyvel wheel {}", wheel.display()),
            );
            let file_name = wheel
                .file_name()
                .ok_or_else(|| format!("invalid wheel path {}", wheel.display()))?;
            fs::copy(wheel, wheelhouse_dir.join(file_name))?;
            Ok(())
        }
        [] => Err(format!(
            "missing local plyvel wheel for {} under {}",
            target.platform,
            target_wheels_dir.display()
        )
        .into()),
        wheels => Err(format!(
            "expected exactly one local plyvel wheel for {}, found {} under {}",
            target.platform,
            wheels.len(),
            target_wheels_dir.display()
        )
        .into()),
    }
}

/// Return locally built `plyvel` wheels under a target wheel directory.
fn find_local_plyvel_wheels(
    target_wheels_dir: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut matches = Vec::new();
    if target_wheels_dir.exists() {
        for entry in fs::read_dir(target_wheels_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if name.starts_with("plyvel-")
                && Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
            {
                matches.push(path);
            }
        }
    }

    Ok(matches)
}

/// Build the local `plyvel` wheel for a platform target.
fn build_local_plyvel_wheel(
    target: &Target,
    build_python: &Path,
    local_wheel_dir: &str,
    target_wheels_dir: &Path,
    plyvel_cross_dir: &Path,
    plyvel_cross_builder: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    log_target(
        target,
        format!(
            "building local plyvel wheel into {}",
            target_wheels_dir.display()
        ),
    );

    let work_dir = plyvel_cross_dir.join(local_wheel_dir);
    let status = Command::new(build_python)
        .arg(plyvel_cross_builder)
        .arg("--target")
        .arg(target.platform)
        .arg("--out-dir")
        .arg(target_wheels_dir)
        .arg("--work-dir")
        .arg(work_dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cross-building plyvel for {} failed with {status}",
            target.platform
        )
        .into())
    }
}

/// Create a directory with standard file system APIs.
fn sh_create_dir_all(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(path)?;
    Ok(())
}

/// Copy already-built wheels into a target wheelhouse.
fn copy_wheels(from: &Path, to: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("whl")) {
            continue;
        }
        let Some(file_name) = path.file_name() else {
            continue;
        };
        fs::copy(&path, to.join(file_name))?;
    }

    Ok(())
}

/// Package a target bundle into its published archive.
fn package_target(
    sh: &Shell,
    target: &Target,
    target_package_dir: &Path,
    dist_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact = dist_dir.join(target.artifact_name);

    let _ = fs::remove_file(&artifact);
    log_target(target, format!("writing {}", artifact.display()));

    let target_package_dir_s = path_string(target_package_dir);
    let artifact_s = path_string(&artifact);
    let exe_name = target.exe_name;
    if target.windows {
        let binary = target_package_dir.join(target.exe_name);
        let binary_s = path_string(&binary);
        cmd!(sh, "zip -j -q {artifact_s} {binary_s}").run()?;
    } else {
        cmd!(
            sh,
            "tar -czf {artifact_s} -C {target_package_dir_s} {exe_name}"
        )
        .run()?;
    }

    verify_archive(&artifact, target)?;
    log_target(target, "archive verified");

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
        dist_dir.join(format!("electrumx-{}-SHA256SUMS", ELECTRUMX_VERSION)),
        format!("{}\n", lines.join("\n")),
    )?;

    Ok(())
}

/// Ensure published archives contain exactly the expected executable.
fn verify_archive(artifact: &Path, target: &Target) -> Result<(), Box<dyn std::error::Error>> {
    let entries = archive_entries(artifact)?;

    if entries.len() != 1 || entries[0] != target.exe_name {
        return Err(format!(
            "{} should contain only {}, got {:?}",
            artifact.display(),
            target.exe_name,
            entries
        )
        .into());
    }

    Ok(())
}

/// Return archive entry names for supported release archive formats.
fn archive_entries(artifact: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if artifact.extension() == Some(OsStr::new("zip")) {
        return Ok(output(Command::new("zip").arg("-sf").arg(artifact))?
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect());
    }

    Ok(output(Command::new("tar").arg("-tzf").arg(artifact))?
        .lines()
        .map(ToOwned::to_owned)
        .collect())
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

/// Return the Python interpreter path inside a virtual environment.
fn venv_python(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

/// Run Python from the build virtual environment without output.
/// Use the specified arguments.
fn run_python(python: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new(python).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {} failed with {status}",
            python.display(),
            args.join(" ")
        )
        .into())
    }
}

/// Run `command` and return UTF-8 `stdout` if it succeeds.
fn output(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!("command failed with {}", output.status).into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

/// Convert a path into an owned string suitable for xshell interpolation.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Print a high-level build step.
fn log_step(message: impl AsRef<str>) {
    println!("==> {}", message.as_ref());
}

/// Print a target-scoped build step.
fn log_target(target: &Target, message: impl AsRef<str>) {
    println!("==> [{}] {}", target.triple, message.as_ref());
}
