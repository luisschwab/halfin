// SPDX-License-Identifier: MIT OR Apache-2.0

use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const ELECTRUMX_VERSION: &str = "1.20.0";
const EMBEDDED_WHEELHOUSE: &[u8] = include_bytes!("../embedded_wheelhouse.bin");
const WHEELHOUSE_MAGIC: &[u8] = b"HALFIN_ELECTRUMX_WHEELHOUSE_V1\0";

#[cfg(not(windows))]
const DEFAULT_PYTHON_COMMAND: &str = "python3.10";
#[cfg(all(windows, target_arch = "x86_64"))]
const DEFAULT_PYTHON_COMMAND: &str = "py -3.10";
#[cfg(all(windows, target_arch = "aarch64"))]
const DEFAULT_PYTHON_COMMAND: &str = "py -3.11";

fn main() {
    if let Err(err) = run() {
        eprintln!("electrumx: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let exe = env::current_exe().map_err(|err| format!("failed to locate executable: {err}"))?;
    let here = exe
        .parent()
        .ok_or_else(|| format!("failed to determine directory for {}", exe.display()))?;
    let runtime_dir = here.join(format!(".electrumx-{ELECTRUMX_VERSION}"));
    let wheelhouse = runtime_dir.join("wheelhouse");
    let venv = runtime_dir.join("venv");
    let entrypoint = electrumx_server(&venv);

    if !entrypoint.is_file() {
        extract_wheelhouse(&wheelhouse)?;
        create_venv(&venv)?;
        install_electrumx(&wheelhouse, &venv)?;
    }

    exec_electrumx(&entrypoint)
}

fn extract_wheelhouse(wheelhouse: &Path) -> Result<(), String> {
    if wheelhouse.is_dir() && wheelhouse_has_wheels(wheelhouse)? {
        return Ok(());
    }

    if wheelhouse.exists() {
        fs::remove_dir_all(wheelhouse).map_err(|err| {
            format!(
                "failed to remove stale wheelhouse {}: {err}",
                wheelhouse.display()
            )
        })?;
    }
    fs::create_dir_all(wheelhouse).map_err(|err| {
        format!(
            "failed to create embedded wheelhouse directory {}: {err}",
            wheelhouse.display()
        )
    })?;

    let mut archive = EmbeddedArchive::new(EMBEDDED_WHEELHOUSE)?;
    for _ in 0..archive.file_count {
        let (name, contents) = archive.next_file()?;
        let path = wheelhouse.join(name);
        fs::write(&path, contents)
            .map_err(|err| format!("failed to write embedded wheel {}: {err}", path.display()))?;
    }

    Ok(())
}

fn wheelhouse_has_wheels(wheelhouse: &Path) -> Result<bool, String> {
    let entries = fs::read_dir(wheelhouse)
        .map_err(|err| format!("failed to read wheelhouse {}: {err}", wheelhouse.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to read wheelhouse entry {}: {err}",
                wheelhouse.display()
            )
        })?;
        if entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn create_venv(venv: &Path) -> Result<(), String> {
    if venv.exists() {
        fs::remove_dir_all(venv).map_err(|err| {
            format!(
                "failed to remove stale Python virtualenv {}: {err}",
                venv.display()
            )
        })?;
    }

    let mut command = python_command();
    command.arg("-m").arg("venv").arg(venv);
    run_command(command, "failed to create private Python virtualenv")
}

fn install_electrumx(wheelhouse: &Path, venv: &Path) -> Result<(), String> {
    let requirement = format!("e-x[leveldb]=={ELECTRUMX_VERSION}");
    let mut command = Command::new(venv_python(venv));
    command
        .arg("-m")
        .arg("pip")
        .arg("install")
        .arg("--no-index")
        .arg("--find-links")
        .arg(wheelhouse)
        .arg(requirement);
    run_command(
        command,
        "failed to install ElectrumX from bundled wheelhouse",
    )
}

fn exec_electrumx(entrypoint: &Path) -> Result<(), String> {
    let mut command = Command::new(entrypoint);
    command.args(apply_flag_env()?);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        Err(command.exec()).map_err(|err| {
            format!(
                "failed to execute ElectrumX entrypoint {}: {err}",
                entrypoint.display()
            )
        })
    }

    #[cfg(not(unix))]
    {
        let status = command.status().map_err(|err| {
            format!(
                "failed to execute ElectrumX entrypoint {}: {err}",
                entrypoint.display()
            )
        })?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn apply_flag_env() -> Result<Vec<OsString>, String> {
    let mut forwarded = Vec::new();
    let mut args = env::args_os().skip(1);

    while let Some(arg) = args.next() {
        let Some(arg_str) = arg.to_str() else {
            forwarded.push(arg);
            continue;
        };

        if arg_str == "--" {
            forwarded.extend(args);
            break;
        }

        if let Some(name) = arg_str.strip_prefix("--") {
            let (name, value) = match name.split_once('=') {
                Some((name, value)) => (name, OsString::from(value)),
                None => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("missing value for --{name}"))?;
                    (name, value)
                }
            };

            if let Some(env_name) = flag_env_name(name) {
                // SAFETY: the launcher is still single-threaded here and sets
                // process environment only before spawning ElectrumX.
                unsafe {
                    env::set_var(env_name, value);
                }
                continue;
            }
        }

        forwarded.push(arg);
    }

    Ok(forwarded)
}

fn flag_env_name(flag: &str) -> Option<&'static str> {
    match flag {
        "db-directory" => Some("DB_DIRECTORY"),
        "db-engine" => Some("DB_ENGINE"),
        "daemon-url" => Some("DAEMON_URL"),
        "coin" => Some("COIN"),
        "net" => Some("NET"),
        "services" => Some("SERVICES"),
        "report-services" => Some("REPORT_SERVICES"),
        "log-level" => Some("LOG_LEVEL"),
        "cache-mb" => Some("CACHE_MB"),
        "peer-discovery" => Some("PEER_DISCOVERY"),
        "peer-announce" => Some("PEER_ANNOUNCE"),
        _ => None,
    }
}

fn python_command() -> Command {
    if let Some(python) = env::var_os("PYTHON") {
        return Command::new(python);
    }

    let mut parts = DEFAULT_PYTHON_COMMAND.split_whitespace();
    let program = parts.next().unwrap_or("python3");
    let mut command = Command::new(program);
    command.args(parts);
    command
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

fn electrumx_server(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("electrumx_server.exe")
    } else {
        venv.join("bin").join("electrumx_server")
    }
}

fn run_command(mut command: Command, context: &str) -> Result<(), String> {
    let display = display_command(&command);
    let status = command
        .status()
        .map_err(|err| format!("{context}: could not run `{display}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{context}: `{display}` exited with {status}"))
    }
}

fn display_command(command: &Command) -> String {
    let mut parts = Vec::new();
    parts.push(command.get_program().to_string_lossy().into_owned());
    parts.extend(command.get_args().map(display_arg));
    parts.join(" ")
}

fn display_arg(arg: &OsStr) -> String {
    let arg = arg.to_string_lossy();
    if arg.contains(char::is_whitespace) {
        format!("{arg:?}")
    } else {
        arg.into_owned()
    }
}

struct EmbeddedArchive<'a> {
    bytes: &'a [u8],
    offset: usize,
    file_count: u32,
}

impl<'a> EmbeddedArchive<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if !bytes.starts_with(WHEELHOUSE_MAGIC) {
            return Err("embedded wheelhouse is missing or corrupt".to_string());
        }

        let mut archive = Self {
            bytes,
            offset: WHEELHOUSE_MAGIC.len(),
            file_count: 0,
        };
        archive.file_count = archive.read_u32()?;
        Ok(archive)
    }

    fn next_file(&mut self) -> Result<(&'a str, &'a [u8]), String> {
        let name_len = self.read_u32()? as usize;
        let contents_len = self.read_u64()? as usize;
        let name = self.read_bytes(name_len)?;
        let contents = self.read_bytes(contents_len)?;
        let name = std::str::from_utf8(name)
            .map_err(|err| format!("embedded wheelhouse has invalid UTF-8 path: {err}"))?;
        if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
            return Err(format!(
                "embedded wheelhouse contains invalid wheel path {name:?}"
            ));
        }
        Ok((name, contents))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("slice length is fixed"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes(
            bytes.try_into().expect("slice length is fixed"),
        ))
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| "embedded wheelhouse offset overflowed".to_string())?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| "embedded wheelhouse ended unexpectedly".to_string())?;
        self.offset = end;
        Ok(bytes)
    }
}
