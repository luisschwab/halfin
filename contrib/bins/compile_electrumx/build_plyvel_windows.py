#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Build target-specific plyvel wheels for ElectrumX bundles."""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import sysconfig
import tarfile
import urllib.request
import zipfile


PYTHON_BUILD_STANDALONE_RELEASE = "20260510"
LEVELDB_VERSION = "1.23"
PLYVEL_VERSION = "1.5.1"


TARGETS = {
    "macosx_11_0_arm64": {
        "kind": "macos",
        "arch": "arm64",
        "deployment_target": "11.0",
        "extension": "_plyvel.cpython-310-darwin.so",
        "tag": "cp310-cp310-macosx_11_0_arm64",
    },
    "macosx_10_9_x86_64": {
        "kind": "macos",
        "arch": "x86_64",
        "deployment_target": "10.9",
        "extension": "_plyvel.cpython-310-darwin.so",
        "tag": "cp310-cp310-macosx_10_9_x86_64",
    },
    "manylinux_2_28_x86_64": {
        "kind": "linux",
        "docker_image": "quay.io/pypa/manylinux_2_28_x86_64",
        "docker_platform": "linux/amd64",
        "python": "/opt/python/cp310-cp310/bin/python",
        "extension": "_plyvel.cpython-310-x86_64-linux-gnu.so",
        "tag": "cp310-cp310-manylinux_2_28_x86_64",
    },
    "manylinux_2_28_aarch64": {
        "kind": "linux",
        "docker_image": "quay.io/pypa/manylinux_2_28_aarch64",
        "docker_platform": "linux/arm64",
        "python": "/opt/python/cp310-cp310/bin/python",
        "extension": "_plyvel.cpython-310-aarch64-linux-gnu.so",
        "tag": "cp310-cp310-manylinux_2_28_aarch64",
    },
    "win_amd64": {
        "kind": "windows",
        "zig_target": "x86_64-windows-gnu",
        "python_archive": "cpython-3.10.20+20260510-x86_64-pc-windows-msvc-install_only_stripped.tar.gz",
        "python_lib": "python310.lib",
        "extension": "_plyvel.cp310-win_amd64.pyd",
        "tag": "cp310-cp310-win_amd64",
    },
    "win_arm64": {
        "kind": "windows",
        "zig_target": "aarch64-windows-gnu",
        "python_archive": "cpython-3.11.15+20260510-aarch64-pc-windows-msvc-install_only_stripped.tar.gz",
        "python_lib": "python311.lib",
        "extension": "_plyvel.cp311-win_arm64.pyd",
        "tag": "cp311-cp311-win_arm64",
    },
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=TARGETS.keys())
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--inside-linux", action="store_true")
    args = parser.parse_args()

    target = TARGETS[args.target]
    args.out_dir.mkdir(parents=True, exist_ok=True)
    args.work_dir.mkdir(parents=True, exist_ok=True)

    if target["kind"] == "linux" and not args.inside_linux:
        build_linux_wheel_in_docker(args.target, args.out_dir, args.work_dir, target)
        return 0

    check_tool("cmake")

    leveldb_dir = prepare_leveldb(args.work_dir)
    enable_leveldb_rtti(leveldb_dir)
    plyvel_dir = prepare_plyvel(args.work_dir)
    if target["kind"] == "windows":
        check_tool("zig")
        python_dir = prepare_windows_python(args.work_dir, target)
        leveldb_build_dir = build_windows_leveldb(args.work_dir, leveldb_dir, target)
        extension = build_windows_extension(args.work_dir, python_dir, leveldb_dir, leveldb_build_dir, plyvel_dir, target)
    elif target["kind"] == "macos":
        leveldb_build_dir = build_macos_leveldb(args.work_dir, leveldb_dir, target)
        extension = build_unix_extension(args.work_dir, leveldb_dir, leveldb_build_dir, plyvel_dir, target)
    elif target["kind"] == "linux":
        leveldb_build_dir = build_linux_leveldb(args.work_dir, leveldb_dir)
        extension = build_unix_extension(args.work_dir, leveldb_dir, leveldb_build_dir, plyvel_dir, target)
    else:
        raise SystemExit(f"unsupported target kind {target['kind']}")
    wheel = build_wheel(args.out_dir, plyvel_dir, extension, target)
    print(wheel)
    return 0


def check_tool(tool: str) -> None:
    if shutil.which(tool) is None:
        raise SystemExit(f"required tool `{tool}` was not found on PATH")


def prepare_windows_python(work_dir: Path, target: dict[str, str]) -> Path:
    archive_name = target["python_archive"]
    archive_path = work_dir / archive_name
    python_dir = work_dir / "python"
    if (python_dir / "include" / "Python.h").is_file() and (python_dir / "libs" / target["python_lib"]).is_file():
        return python_dir

    shutil.rmtree(python_dir, ignore_errors=True)
    url = (
        "https://github.com/astral-sh/python-build-standalone/releases/download/"
        f"{PYTHON_BUILD_STANDALONE_RELEASE}/{archive_name.replace('+', '%2B')}"
    )
    download(url, archive_path)
    extract_tar_gz(archive_path, work_dir)
    return python_dir


def prepare_leveldb(work_dir: Path) -> Path:
    source_dir = work_dir / f"leveldb-{LEVELDB_VERSION}"
    if (source_dir / "CMakeLists.txt").is_file():
        return source_dir

    archive = work_dir / f"leveldb-{LEVELDB_VERSION}.tar.gz"
    url = f"https://github.com/google/leveldb/archive/refs/tags/{LEVELDB_VERSION}.tar.gz"
    download(url, archive)
    extract_tar_gz(archive, work_dir)
    return source_dir


def enable_leveldb_rtti(source_dir: Path) -> None:
    cmake_lists = source_dir / "CMakeLists.txt"
    contents = cmake_lists.read_text()
    contents = contents.replace("/GR-", "/GR")
    contents = contents.replace("-fno-rtti", "-frtti")
    cmake_lists.write_text(contents)


def prepare_plyvel(work_dir: Path) -> Path:
    source_dir = work_dir / f"plyvel-{PLYVEL_VERSION}"
    if (source_dir / "setup.py").is_file():
        return source_dir

    subprocess.run(
        [
            sys.executable,
            "-m",
            "pip",
            "download",
            "--dest",
            str(work_dir),
            "--no-binary=:all:",
            "--no-deps",
            f"plyvel=={PLYVEL_VERSION}",
        ],
        check=True,
    )
    extract_tar_gz(work_dir / f"plyvel-{PLYVEL_VERSION}.tar.gz", work_dir)
    return source_dir


def build_linux_wheel_in_docker(target_name: str, out_dir: Path, work_dir: Path, target: dict[str, str]) -> None:
    check_tool("docker")
    repo_root = Path.cwd().resolve()
    script = Path(__file__).resolve()
    out_dir = out_dir.resolve()
    work_dir = work_dir.resolve()
    for path in [script, out_dir, work_dir]:
        try:
            path.relative_to(repo_root)
        except ValueError as exc:
            raise SystemExit(f"{path} must be under repository root {repo_root}") from exc

    def container_path(path: Path) -> str:
        return "/work/" + path.relative_to(repo_root).as_posix()

    subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--platform",
            target["docker_platform"],
            "-v",
            f"{repo_root}:/work",
            "-w",
            "/work",
            target["docker_image"],
            target["python"],
            container_path(script),
            "--target",
            target_name,
            "--out-dir",
            container_path(out_dir),
            "--work-dir",
            container_path(work_dir),
            "--inside-linux",
        ],
        check=True,
    )


def build_windows_leveldb(work_dir: Path, source_dir: Path, target: dict[str, str]) -> Path:
    build_dir = work_dir / "leveldb-build"
    if (build_dir / "libleveldb.a").is_file():
        return build_dir

    shutil.rmtree(build_dir, ignore_errors=True)
    wrappers = work_dir / "zig-wrap"
    wrappers.mkdir(parents=True, exist_ok=True)
    cc = wrappers / "zig-cc"
    cxx = wrappers / "zig-cxx"
    write_wrapper(cc, "cc", target["zig_target"], work_dir)
    write_wrapper(cxx, "c++", target["zig_target"], work_dir)

    env = zig_env(work_dir)
    subprocess.run(
        [
            "cmake",
            "-S",
            str(source_dir),
            "-B",
            str(build_dir),
            "-DCMAKE_SYSTEM_NAME=Windows",
            f"-DCMAKE_C_COMPILER={cc}",
            f"-DCMAKE_CXX_COMPILER={cxx}",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DCMAKE_CXX_FLAGS=-frtti",
            "-DLEVELDB_BUILD_TESTS=OFF",
            "-DLEVELDB_BUILD_BENCHMARKS=OFF",
        ],
        env=env,
        check=True,
    )
    subprocess.run(["cmake", "--build", str(build_dir), "--config", "Release"], env=env, check=True)
    return build_dir


def build_macos_leveldb(work_dir: Path, source_dir: Path, target: dict[str, str]) -> Path:
    build_dir = work_dir / "leveldb-build"
    if (build_dir / "libleveldb.a").is_file():
        return build_dir

    shutil.rmtree(build_dir, ignore_errors=True)
    subprocess.run(
        [
            "cmake",
            "-S",
            str(source_dir),
            "-B",
            str(build_dir),
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DLEVELDB_BUILD_TESTS=OFF",
            "-DLEVELDB_BUILD_BENCHMARKS=OFF",
            "-DCMAKE_C_COMPILER=/usr/bin/clang",
            "-DCMAKE_CXX_COMPILER=/usr/bin/clang++",
            "-DCMAKE_CXX_FLAGS=-frtti",
            f"-DCMAKE_OSX_ARCHITECTURES={target['arch']}",
            f"-DCMAKE_OSX_DEPLOYMENT_TARGET={target['deployment_target']}",
        ],
        check=True,
    )
    subprocess.run(["cmake", "--build", str(build_dir), "--config", "Release"], check=True)
    return build_dir


def build_linux_leveldb(work_dir: Path, source_dir: Path) -> Path:
    build_dir = work_dir / "leveldb-build"
    if (build_dir / "libleveldb.a").is_file():
        return build_dir

    shutil.rmtree(build_dir, ignore_errors=True)
    subprocess.run(
        [
            "cmake",
            "-S",
            str(source_dir),
            "-B",
            str(build_dir),
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DCMAKE_CXX_FLAGS=-frtti",
            "-DLEVELDB_BUILD_TESTS=OFF",
            "-DLEVELDB_BUILD_BENCHMARKS=OFF",
        ],
        check=True,
    )
    subprocess.run(["cmake", "--build", str(build_dir), "--config", "Release"], check=True)
    return build_dir


def write_wrapper(path: Path, zig_command: str, zig_target: str, work_dir: Path) -> None:
    path.write_text(
        "#!/bin/sh\n"
        f"export ZIG_LOCAL_CACHE_DIR={work_dir / 'zig-cache' / 'local'}\n"
        f"export ZIG_GLOBAL_CACHE_DIR={work_dir / 'zig-cache' / 'global'}\n"
        f'exec zig {zig_command} -target {zig_target} "$@"\n'
    )
    path.chmod(0o755)


def build_windows_extension(
    work_dir: Path,
    python_dir: Path,
    leveldb_dir: Path,
    leveldb_build_dir: Path,
    plyvel_dir: Path,
    target: dict[str, str],
) -> Path:
    build_dir = work_dir / "plyvel-build"
    build_dir.mkdir(parents=True, exist_ok=True)
    env = zig_env(work_dir)
    includes = [
        f"-I{plyvel_dir}",
        f"-I{python_dir / 'include'}",
        f"-I{leveldb_dir / 'include'}",
    ]
    common = [
        "zig",
        "c++",
        "-target",
        target["zig_target"],
        "-O2",
        "-std=c++11",
        "-DMS_WIN64",
        *includes,
    ]
    objects = [
        (plyvel_dir / "plyvel" / "_plyvel.cpp", build_dir / "_plyvel.obj"),
        (plyvel_dir / "plyvel" / "comparator.cpp", build_dir / "comparator.obj"),
    ]
    for source, obj in objects:
        subprocess.run([*common, "-c", str(source), "-o", str(obj)], env=env, check=True)

    extension = build_dir / target["extension"]
    subprocess.run(
        [
            "zig",
            "c++",
            "-target",
            target["zig_target"],
            "-shared",
            str(build_dir / "_plyvel.obj"),
            str(build_dir / "comparator.obj"),
            str(leveldb_build_dir / "libleveldb.a"),
            str(python_dir / "libs" / target["python_lib"]),
            "-o",
            str(extension),
        ],
        env=env,
        check=True,
    )
    return extension


def build_unix_extension(
    work_dir: Path,
    leveldb_dir: Path,
    leveldb_build_dir: Path,
    plyvel_dir: Path,
    target: dict[str, str],
) -> Path:
    build_dir = work_dir / "plyvel-build"
    build_dir.mkdir(parents=True, exist_ok=True)
    include_dir = sysconfig.get_path("include")
    includes = [
        f"-I{plyvel_dir}",
        f"-I{include_dir}",
        f"-I{leveldb_dir / 'include'}",
    ]
    common = [
        cxx_compiler(target),
        "-O2",
        "-fPIC",
        "-Wall",
        "-g",
        "-x",
        "c++",
        "-std=c++11",
        *unix_arch_flags(target),
        *includes,
    ]
    if target["kind"] == "macos":
        common.append("-stdlib=libc++")

    objects = [
        (plyvel_dir / "plyvel" / "_plyvel.cpp", build_dir / "_plyvel.o"),
        (plyvel_dir / "plyvel" / "comparator.cpp", build_dir / "comparator.o"),
    ]
    for source, obj in objects:
        subprocess.run([*common, "-c", str(source), "-o", str(obj)], check=True)

    extension = build_dir / target["extension"]
    if target["kind"] == "macos":
        subprocess.run(
            [
                cxx_compiler(target),
                "-bundle",
                "-undefined",
                "dynamic_lookup",
                *unix_arch_flags(target),
                str(build_dir / "_plyvel.o"),
                str(build_dir / "comparator.o"),
                str(leveldb_build_dir / "libleveldb.a"),
                "-stdlib=libc++",
                "-o",
                str(extension),
            ],
            check=True,
        )
    else:
        subprocess.run(
            [
                cxx_compiler(target),
                "-shared",
                str(build_dir / "_plyvel.o"),
                str(build_dir / "comparator.o"),
                str(leveldb_build_dir / "libleveldb.a"),
                "-pthread",
                "-o",
                str(extension),
            ],
            check=True,
        )
    return extension


def cxx_compiler(target: dict[str, str]) -> str:
    if target["kind"] == "macos":
        return "/usr/bin/clang++"
    return "c++"


def unix_arch_flags(target: dict[str, str]) -> list[str]:
    if target["kind"] != "macos":
        return []
    return [
        "-arch",
        target["arch"],
        f"-mmacosx-version-min={target['deployment_target']}",
    ]


def build_wheel(out_dir: Path, plyvel_dir: Path, extension: Path, target: dict[str, str]) -> Path:
    tag = target["tag"]
    dist_info = f"plyvel-{PLYVEL_VERSION}.dist-info"
    wheel_name = f"plyvel-{PLYVEL_VERSION}-{tag}.whl"
    wheel_path = out_dir / wheel_name
    if wheel_path.exists():
        wheel_path.unlink()

    files: dict[str, bytes] = {}
    for name in ["__init__.py", "_version.py"]:
        files[f"plyvel/{name}"] = (plyvel_dir / "plyvel" / name).read_bytes()
    files[f"plyvel/{extension.name}"] = extension.read_bytes()
    files[f"{dist_info}/METADATA"] = (plyvel_dir / "PKG-INFO").read_bytes()
    files[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: halfin-cross-plyvel\n"
        "Root-Is-Purelib: false\n"
        f"Tag: {tag}\n"
    ).encode()
    files[f"{dist_info}/top_level.txt"] = b"plyvel\n"
    files[f"{dist_info}/LICENSE.rst"] = (plyvel_dir / "LICENSE.rst").read_bytes()

    records = []
    with zipfile.ZipFile(wheel_path, "w", compression=zipfile.ZIP_DEFLATED) as wheel:
        for name, data in sorted(files.items()):
            wheel.writestr(name, data)
            records.append(f"{name},sha256={hash_record(data)},{len(data)}")
        record_name = f"{dist_info}/RECORD"
        records.append(f"{record_name},,")
        wheel.writestr(record_name, "\n".join(records).encode() + b"\n")

    return wheel_path


def hash_record(data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode()


def download(url: str, path: Path) -> None:
    if path.is_file():
        return
    print(f"downloading {url}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url) as response:
        path.write_bytes(response.read())


def extract_tar_gz(archive: Path, directory: Path) -> None:
    with tarfile.open(archive, "r:gz") as tar:
        tar.extractall(directory)


def zig_env(work_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["ZIG_LOCAL_CACHE_DIR"] = str(work_dir / "zig-cache" / "local")
    env["ZIG_GLOBAL_CACHE_DIR"] = str(work_dir / "zig-cache" / "global")
    return env


if __name__ == "__main__":
    raise SystemExit(main())
