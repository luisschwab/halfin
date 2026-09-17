// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build four `libbitcoin-server` archives on an Apple Silicon Mac.
//! Run `just compile-bins compile-libbitcoin` from the repository root.

#[cfg(target_os = "macos")]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/node/libbitcoind/versions.rs"
));

#[cfg(target_os = "macos")]
use std::env;
#[cfg(target_os = "macos")]
use std::error::Error;
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use xshell::Shell;
#[cfg(target_os = "macos")]
use xshell::cmd;

/// Full commits used for all libbitcoin source repositories.
#[cfg(target_os = "macos")]
const SOURCES: &[(&str, &str)] = &[
    (
        "libbitcoin-system",
        "aca57212c28a71626e95ed617ad749f2db36abf0",
    ),
    (
        "libbitcoin-database",
        "8db26e1684e21d62ccc202559a1d00dad435beba",
    ),
    (
        "libbitcoin-network",
        "e20491167a0e122075fad7238041968f3cd25852",
    ),
    (
        "libbitcoin-node",
        "a5c48d3f1b911a582c72d298d60be978677a241c",
    ),
    ("libbitcoin-server", LIBBITCOIN_VERSION),
];

/// Linux toolchain image definition passed to `docker build` on standard input.
#[cfg(target_os = "macos")]
const DOCKERFILE: &str = "\
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \\
    autoconf automake build-essential bzip2 ca-certificates curl file git libtool pkg-config \\
    && rm -rf /var/lib/apt/lists/*
";

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
/// One platform artifact produced by the builder.
struct Target {
    /// Rust-style target triple used to identify the target.
    triple: &'static str,
    /// Published archive file name.
    archive: &'static str,
    /// Container platform for Linux targets.
    docker_platform: Option<&'static str>,
}

/// The complete supported artifact set.
#[cfg(target_os = "macos")]
const TARGETS: &[Target] = &[
    Target {
        triple: "aarch64-apple-darwin",
        archive: "libbitcoin-darwin-arm64.tar.gz",
        docker_platform: None,
    },
    Target {
        triple: "x86_64-apple-darwin",
        archive: "libbitcoin-darwin-amd64.tar.gz",
        docker_platform: None,
    },
    Target {
        triple: "aarch64-unknown-linux-gnu",
        archive: "libbitcoin-linux-arm64.tar.gz",
        docker_platform: Some("linux/arm64"),
    },
    Target {
        triple: "x86_64-unknown-linux-gnu",
        archive: "libbitcoin-linux-amd64.tar.gz",
        docker_platform: Some("linux/amd64"),
    },
];

/// Build all missing archives and write their checksums.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn Error>> {
    let sh = Shell::new()?;
    let mut force = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            "--help" | "-h" => {
                println!("Usage: compile-libbitcoin [--force]");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if env::consts::OS != "macos" {
        return Err("building all four targets requires a macOS host".into());
    }
    if env::consts::ARCH != "aarch64" {
        return Err(
            "building both macOS architectures requires an Apple Silicon Mac with Rosetta 2".into(),
        );
    }
    for tool in [
        "git",
        "autoconf",
        "automake",
        "pkg-config",
        "glibtoolize",
        "file",
        "otool",
    ] {
        if cmd!(sh, "which {tool}").read().is_err() {
            return Err(format!(
                "missing macOS build prerequisite `{tool}`; see compile_libbitcoin/README.md"
            )
            .into());
        }
    }
    cmd!(sh, "docker info").run()?;
    if env::consts::ARCH == "aarch64" {
        cmd!(sh, "arch -x86_64 /usr/bin/true")
            .run()
            .map_err(|_| "Rosetta 2 is required for the macOS x86_64 build")?;
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/bins/compile_libbitcoin");
    let dist = root
        .join("dist")
        .join(format!("libbitcoin-{LIBBITCOIN_VERSION}"));
    fs::create_dir_all(&dist)?;
    for target in TARGETS {
        let archive = dist.join(target.archive);
        if archive.exists() && !force {
            println!("Reusing {}", archive.display());
            continue;
        }
        let work = root.join("tmp").join(target.triple);
        fs::create_dir_all(&work)?;
        fs::create_dir_all(work.join("home"))?;
        for (repository, commit) in SOURCES {
            prepare_source(&sh, &work, repository, commit)?;
        }
        let executable = build_target(&sh, &work, *target)?;
        verify_executable(&sh, &work, &executable, *target)?;
        package(&sh, &executable, &archive)?;
        println!("Built {}", archive.display());
    }
    write_checksums(&dist)?;
    println!(
        "Four archives and SHA256SUMS are ready in {}",
        dist.display()
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Building libbitcoin archives requires an Apple Silicon Mac");
    std::process::exit(1);
}

/// Check out and verify one pinned source repository.
#[cfg(target_os = "macos")]
fn prepare_source(
    sh: &Shell,
    work: &Path,
    repository: &str,
    commit: &str,
) -> Result<(), Box<dyn Error>> {
    let path = work.join(repository);
    let url = format!("https://github.com/libbitcoin/{repository}.git");
    let path_s = path.display().to_string();
    if !path.exists() {
        cmd!(sh, "git clone --no-checkout {url} {path_s}").run_echo()?;
    }
    let source = sh.with_current_dir(&path);
    if cmd!(source, "git remote get-url origin").read()?.trim() != url {
        return Err(format!("unexpected origin for {}", path.display()).into());
    }
    cmd!(source, "git fetch origin {commit}").run_echo()?;
    cmd!(source, "git checkout --force {commit}").run_echo()?;
    if cmd!(source, "git rev-parse HEAD").read()?.trim() != commit {
        return Err(format!("commit mismatch for {repository}").into());
    }
    Ok(())
}

/// Run a short command in the target Linux container.
#[cfg(target_os = "macos")]
fn docker(
    sh: &Shell,
    work: &Path,
    target: Target,
    program: &str,
    args: &[&str],
) -> Result<String, Box<dyn Error>> {
    let platform = target.docker_platform.ok_or("Docker target expected")?;
    let user = format!(
        "{}:{}",
        cmd!(sh, "id -u").read()?,
        cmd!(sh, "id -g").read()?
    );
    let mount = format!("{}:/work", work.display());
    Ok(cmd!(sh, "docker run")
        .args([
            "--rm",
            "--platform",
            platform,
            "--user",
            &user,
            "-e",
            "HOME=/work/home",
            "-v",
            &mount,
            "-w",
            "/work/libbitcoin-server",
            "halfin-libbitcoin-builder:ubuntu24",
            program,
        ])
        .args(args)
        .read()?)
}

/// Build the release executable for one target.
#[cfg(target_os = "macos")]
fn build_target(sh: &Shell, work: &Path, target: Target) -> Result<PathBuf, Box<dyn Error>> {
    let server = work.join("libbitcoin-server");
    if let Some(platform) = target.docker_platform {
        // Build a reusable toolchain image. Docker selects the architecture at run time.
        // xshell's run_echo() closes stdin, so use run() for the inline Dockerfile.
        println!("Building Linux toolchain image for {platform}");
        cmd!(
            sh,
            "docker build --platform {platform} -t halfin-libbitcoin-builder:ubuntu24 -"
        )
        .stdin(DOCKERFILE)
        .run()?;
        let user = format!(
            "{}:{}",
            cmd!(sh, "id -u").read()?,
            cmd!(sh, "id -g").read()?
        );
        let mount = format!("{}:/work", work.display());
        // Docker Desktop has a separate memory limit from the host. Four jobs
        // fit the configured 16 GB VM; the upstream CPU-count default did not.
        cmd!(sh, "docker run")
            .args([
                "--rm",
                "--platform",
                platform,
                "--user",
                &user,
                "-e",
                "HOME=/work/home",
                "-v",
                &mount,
                "-w",
                "/work/libbitcoin-server",
                "halfin-libbitcoin-builder:ubuntu24",
                "./builds/gnu/install-gnu.sh",
            ])
            .args([
                "--build-src-dir=/work",
                "--prefix=/work/prefix",
                "--build-use-local-src",
                "--build-secp256k1",
                "--build-boost",
                "--build-config=release",
                "--build-link=static",
                "--build-parallel=4",
                "--build-skip-tests",
                "--noninteractive",
            ])
            .run_echo()?;
    } else {
        let prefix = work.join("prefix");
        let developer_dir = macos_developer_dir();
        let apple_path = macos_compiler_path(sh, work, target, developer_dir.as_deref())?;
        let source = sh.with_current_dir(&server);
        let script = server.join("builds/gnu/install-gnu.sh");
        let script_s = script.display().to_string();
        let build_src_dir = format!("--build-src-dir={}", work.display());
        let prefix_arg = format!("--prefix={}", prefix.display());

        let mut command = cmd!(
            source,
            "arch -arm64 bash {script_s} {build_src_dir} {prefix_arg}"
        )
        .args([
            "--build-use-local-src",
            "--build-secp256k1",
            "--build-boost",
            "--build-config=release",
            "--build-link=static",
            "--build-parallel=8",
            "--build-skip-tests",
            "--noninteractive",
        ])
        .env("CC", "clang")
        .env("CXX", "clang++")
        .env("PATH", apple_path);
        if let Some(developer_dir) = developer_dir {
            command = command.env("DEVELOPER_DIR", developer_dir.display().to_string());
        }
        command.run_echo()?;
    }
    let bin = work.join("prefix/bin/bs");
    if !bin.is_file() {
        return Err(format!("missing built executable: {}", bin.display()).into());
    }
    Ok(bin)
}

/// Prefer the installed Xcode toolchain while honoring an explicit selection.
#[cfg(target_os = "macos")]
fn macos_developer_dir() -> Option<PathBuf> {
    env::var_os("DEVELOPER_DIR").map(PathBuf::from).or_else(|| {
        let xcode = PathBuf::from("/Applications/Xcode.app/Contents/Developer");
        xcode.is_dir().then_some(xcode)
    })
}

/// Resolve a tool using the selected Apple developer directory.
#[cfg(target_os = "macos")]
fn xcrun(
    sh: &Shell,
    developer_dir: Option<&Path>,
    args: &[&str],
) -> Result<String, Box<dyn Error>> {
    let mut command = cmd!(sh, "xcrun {args...}");
    if let Some(developer_dir) = developer_dir {
        command = command.env("DEVELOPER_DIR", developer_dir.display().to_string());
    }
    Ok(command.read()?)
}

/// Select Apple Clang and provide `x86_64` wrappers when building through Rosetta.
#[cfg(target_os = "macos")]
fn macos_compiler_path(
    sh: &Shell,
    work: &Path,
    target: Target,
    developer_dir: Option<&Path>,
) -> Result<String, Box<dyn Error>> {
    // Homebrew LLVM 23 rejects ambiguous conversions in this pinned source.
    // Keep CC/CXX as toolset names for Boost, but resolve them to Apple Clang.
    if !target.triple.starts_with("x86_64") {
        return Ok(format!("/usr/bin:/bin:{}", env::var("PATH")?));
    }

    let wrappers = work.join("toolchain/bin");
    fs::create_dir_all(&wrappers)?;
    let sdk_path = xcrun(sh, developer_dir, &["--show-sdk-path"])?;
    let sdk_path = sdk_path.trim();
    for compiler in ["clang", "clang++"] {
        let wrapper = wrappers.join(compiler);
        // Boost's b2 runs under Rosetta. /usr/bin/clang invokes an x86_64
        // xcrun there, so invoke the actual ARM64 compiler and pass the SDK.
        let compiler_path = xcrun(sh, developer_dir, &["--find", compiler])?;
        let compiler_path = compiler_path.trim();
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec \"{compiler_path}\" -arch x86_64 -isysroot \"{sdk_path}\" \"$@\"\n"
            ),
        )?;
        let wrapper_s = wrapper.display().to_string();
        cmd!(sh, "chmod +x {wrapper_s}").run()?;
    }
    // Boost's x86_64 b2 also launches archivers. Their /usr/bin shims use an
    // x86_64 xcrun that this ARM-only Command Line Tools install cannot load.
    for tool in ["libtool", "ar", "ranlib", "strip", "ld", "lipo", "nm"] {
        let tool_path = xcrun(sh, developer_dir, &["--find", tool])?;
        let wrapper = wrappers.join(tool);
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", tool_path.trim()),
        )?;
        let wrapper_s = wrapper.display().to_string();
        cmd!(sh, "chmod +x {wrapper_s}").run()?;
    }
    Ok(format!(
        "{}:/usr/bin:/bin:{}",
        wrappers.display(),
        env::var("PATH")?
    ))
}

/// Check the executable format, startup commands, and linked libraries.
#[cfg(target_os = "macos")]
fn verify_executable(
    sh: &Shell,
    work: &Path,
    bin: &Path,
    target: Target,
) -> Result<(), Box<dyn Error>> {
    let bin_s = bin.display().to_string();
    let details = cmd!(sh, "file {bin_s}").read()?.to_lowercase();
    let expected = if target.triple.starts_with("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let accepted = details.contains(expected)
        || (expected == "arm64" && details.contains("aarch64"))
        || (expected == "x86_64" && details.contains("x86-64"));
    if !accepted {
        return Err(format!("wrong executable architecture: {details}").into());
    }
    for arg in ["--version", "--help", "--hardware"] {
        if target.docker_platform.is_some() {
            docker(sh, work, target, "/work/prefix/bin/bs", &[arg])?;
        } else {
            let arch = if target.triple.starts_with("x86_64") {
                "-x86_64"
            } else {
                "-arm64"
            };
            cmd!(sh, "arch {arch} {bin_s} {arg}").run()?;
        }
    }
    let deps = if target.docker_platform.is_some() {
        docker(sh, work, target, "ldd", &["/work/prefix/bin/bs"])?
    } else {
        cmd!(sh, "otool -L {bin_s}").read()?
    };
    let deps = deps.to_lowercase();
    if deps.contains("not found") || deps.contains("libbitcoin-") || deps.contains("libboost_") {
        return Err(format!("unbundled library dependency: {deps}").into());
    }
    Ok(())
}

/// Package a verified executable in a single-file archive.
#[cfg(target_os = "macos")]
fn package(sh: &Shell, bin: &Path, archive: &Path) -> Result<(), Box<dyn Error>> {
    let stage = archive
        .parent()
        .ok_or("archive has no parent")?
        .join("stage");
    fs::create_dir_all(&stage)?;
    fs::copy(bin, stage.join("bs"))?;
    let tmp = archive.with_extension("partial.tar.gz");
    let tmp_s = tmp.display().to_string();
    let stage_s = stage.display().to_string();
    cmd!(sh, "tar -czf {tmp_s} -C {stage_s} bs").run()?;
    if cmd!(sh, "tar -tzf {tmp_s}").read()?.trim() != "bs" {
        return Err("archive contains unexpected files".into());
    }
    if archive.exists() {
        fs::remove_file(archive)?;
    }
    fs::rename(tmp, archive)?;
    fs::remove_dir_all(stage)?;
    Ok(())
}

/// Write checksums after all target archives exist.
#[cfg(target_os = "macos")]
fn write_checksums(dist: &Path) -> Result<(), Box<dyn Error>> {
    let mut lines = String::new();
    for target in TARGETS {
        let path = dist.join(target.archive);
        if !path.is_file() {
            return Err(format!("missing archive: {}", path.display()).into());
        }
        lines.push_str(&format!(
            "{}  {}\n",
            bitcoin_hashes::sha256::Hash::hash(&fs::read(path)?),
            target.archive
        ));
    }
    fs::write(
        dist.join(format!("libbitcoin-{LIBBITCOIN_VERSION}-SHA256SUMS")),
        lines,
    )?;
    Ok(())
}
