#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Actual Tauri DEB payload extraction, rootfs merge and ELF audit."""

import io
import os
import re
import shutil
import tarfile
from pathlib import Path

from common import APP, PackagingError, regular_bytes, run, safe_relative

REQUIRED = (
    "usr/bin/peripheral-hub", "usr/bin/peripheral-hub-remapping-agent",
    "usr/lib/systemd/user/peripheral-hub-remapping-agent.service",
    "usr/share/applications/io.peripheralhub.Recovery.desktop",
    "usr/lib/peripheral-hub/remapper/bridge.py",
    "usr/lib/peripheral-hub/remapper/vendor/inputremapper/injection/injector.py",
    "usr/lib/peripheral-hub/remapper-service",
    "usr/lib/systemd/system/peripheral-hub-input-remapper.service",
    "usr/share/dbus-1/system.d/io.peripheralhub.InputRemapper.conf",
    "usr/share/doc/peripheral-hub/remapper/LICENSE.input-remapper",
    "usr/lib/peripheral-hub/installer/native.py",
    "usr/lib/systemd/system-preset/90-peripheral-hub.preset",
    "etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop",
)


def extract_tauri_deb(package, rootfs):
    data = regular_bytes(package, 2 ** 31)
    if not data.startswith(b"!<arch>\n"):
        raise PackagingError("Tauri did not produce a DEB ar archive")
    offset = 8
    payload = None
    while offset < len(data):
        header = data[offset:offset + 60]
        if len(header) != 60 or header[58:] != b"`\n":
            raise PackagingError("Invalid DEB member header")
        name = header[:16].decode("ascii").strip().rstrip("/")
        size = int(header[48:58].strip())
        offset += 60
        contents = data[offset:offset + size]
        offset += size + (size % 2)
        if len(contents) != size:
            raise PackagingError("Truncated Tauri DEB")
        if name.startswith("data.tar"):
            if payload is not None:
                raise PackagingError("Duplicate DEB payload")
            payload = contents
    if payload is None:
        raise PackagingError("Tauri DEB lacks a filesystem payload")
    # Tauri uses gzip. Newer compression is deliberately not guessed or executed.
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
        seen = set()
        for member in archive:
            name = member.name.removeprefix("./").rstrip("/")
            if not name:
                continue
            relative = safe_relative(name)
            if relative.parts[0] != "usr":
                raise PackagingError("Unexpected Tauri filesystem outside /usr")
            destination = Path(rootfs).joinpath(*relative.parts)
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile() or name in seen:
                raise PackagingError("Tauri payload contains a link/special/duplicate file")
            seen.add(name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(archive.extractfile(member).read())
            destination.chmod(0o755 if member.mode & 0o111 else 0o644)


def merge_rootfs(source, destination):
    for path in sorted(Path(source).rglob("*")):
        relative = path.relative_to(source)
        target = Path(destination) / relative
        if path.is_symlink():
            raise PackagingError(f"Rootfs resource may not be a symlink: {relative}")
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        data = regular_bytes(path, 64 * 1024 * 1024)
        if target.exists() and regular_bytes(target, len(data) + 1) != data:
            raise PackagingError(f"Conflicting staged resource: {relative}")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(0o755 if path.name == "remapper-service" else 0o644)


def elf_report(path):
    header = run(["readelf", "--file-header", str(path)]).stdout
    if not re.search(r"Class:\s+ELF64", header) or not re.search(r"Machine:\s+Advanced Micro Devices X86-64", header):
        raise PackagingError(f"Not an x86_64 ELF binary: {path}")
    dynamic = run(["readelf", "--dynamic", str(path)]).stdout
    versions = run(["readelf", "--version-info", str(path)]).stdout
    program = run(["readelf", "--program-headers", str(path)]).stdout
    loader = run(["ldd", str(path)], check=False)
    if loader.returncode or "not found" in loader.stdout or "not found" in loader.stderr:
        raise PackagingError(f"ELF dependencies do not resolve on this target: {path}: {loader.stdout} {loader.stderr}")
    providers = re.findall(r"=>\s+(/\S+)", loader.stdout)
    return {
        "needed": re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic),
        "interpreter": next(iter(re.findall(r"Requesting program interpreter:\s*([^\]]+)", program)), None),
        "symbols": sorted(set(re.findall(r"Name:\s+((?:GLIBC|GLIBCXX|CXXABI)_[\d.]+)", versions))),
        "resolved_libraries": providers,
    }


def audit(rootfs):
    rootfs = Path(rootfs)
    missing = [name for name in REQUIRED if not (rootfs / name).is_file()]
    if missing:
        raise PackagingError("Complete rootfs is missing: " + ", ".join(missing))
    agent = (rootfs / "usr/lib/systemd/user/peripheral-hub-remapping-agent.service").read_text()
    if "ExecStart=/usr/bin/peripheral-hub-remapping-agent\n" not in agent:
        raise PackagingError("Agent unit does not use the agreed absolute Cargo binary")
    reports = {}
    for path in sorted(rootfs.rglob("*")):
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            is_elf = stream.read(4) == b"\x7fELF"
        if is_elf:
            reports[path.relative_to(rootfs).as_posix()] = elf_report(path)
    for name in ("usr/bin/peripheral-hub", "usr/bin/peripheral-hub-remapping-agent"):
        if name not in reports:
            raise PackagingError(f"Mandatory executable is not ELF: {name}")
    agent_needed = reports["usr/bin/peripheral-hub-remapping-agent"]["needed"]
    if any("gtk" in name or "webkit" in name for name in agent_needed):
        raise PackagingError("Agent linked desktop libraries; build must use --no-default-features")
    return reports


def rootfs_archive(rootfs, destination, epoch):
    import gzip
    with Path(destination).open("wb") as output:
        with gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for path in sorted(Path(rootfs).rglob("*")):
                    relative = path.relative_to(rootfs).as_posix()
                    info = archive.gettarinfo(str(path), arcname=relative)
                    info.uid = info.gid = 0
                    info.uname = info.gname = "root"
                    info.mtime = epoch
                    if path.is_dir():
                        info.mode = 0o755
                        archive.addfile(info)
                    else:
                        with path.open("rb") as stream:
                            archive.addfile(info, stream)
