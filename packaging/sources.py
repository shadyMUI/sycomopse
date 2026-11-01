#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Pinned source fetch, private builds and corresponding-source archives."""

import gzip
import inspect
import io
import os
import shutil
import tarfile
import zipfile
from pathlib import Path, PurePosixPath

from common import HERE, PackagingError, digest, https_bytes, json_file, probe_headset, regular_bytes, require_build_user, run, safe_relative


def extract_source(data, destination):
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    seen = set()
    total = 0
    root = None
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            name = safe_relative(member.name.rstrip("/"))
            if root is None:
                root = name.parts[0]
            if name.parts[0] != root:
                raise PackagingError("Source archive must have one top-level directory")
            if member.isdir():
                continue
            relative = PurePosixPath(*name.parts[1:])
            if not member.isfile() or not relative.parts or relative in seen:
                raise PackagingError("Links, special files and duplicate source members are forbidden")
            total += member.size
            if member.size < 0 or total > 128 * 1024 * 1024:
                raise PackagingError("Source archive exceeds expanded size limit")
            seen.add(relative)
            stream = archive.extractfile(member)
            contents = stream.read(member.size + 1)
            if len(contents) != member.size:
                raise PackagingError("Truncated source archive")
            path = destination.joinpath(*relative.parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
            path.chmod(0o755 if member.mode & 0o111 else 0o644)
    return destination


def fetch_locked(entry, cache, name):
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / name
    data = regular_bytes(path, 32 * 1024 * 1024) if path.exists() else https_bytes(entry["url"])
    if digest(data) != entry["sha256"]:
        raise PackagingError(f"Pinned source checksum mismatch: {name}")
    if not path.exists():
        path.write_bytes(data)
    return data


def evdev_private(rootfs, work, artifacts, cache):
    require_build_user()
    try:
        import evdev
        usable = "input_props" in inspect.signature(evdev.UInput).parameters
    except ImportError:
        usable = False
    if usable:
        return {"private": False, "reason": "Distro evdev exposes UInput.input_props"}
    lock = json_file(HERE / "python-sources.lock.json")["evdev"]
    data = fetch_locked(lock, cache, f"evdev-{lock['version']}.tar.gz")
    source = extract_source(data, Path(work) / "evdev-source")
    # setuptools comes from signed distro packages; no pip, wheel download or root build.
    run(["python3", "setup.py", "build", "--build-base", str(Path(work) / "evdev-build")], cwd=source, capture=False, timeout=300)
    built = list((Path(work) / "evdev-build").glob("lib*/evdev"))
    if len(built) != 1:
        raise PackagingError("Private evdev build did not produce exactly one module")
    destination = Path(rootfs) / "usr/lib/peripheral-hub/remapper/vendor/evdev"
    shutil.copytree(built[0], destination)
    docs = Path(rootfs) / "usr/share/doc/peripheral-hub/evdev"
    docs.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / "LICENSE", docs / "LICENSE")
    archive = Path(artifacts) / f"evdev-{lock['version']}-source.tar.gz"
    archive.write_bytes(data)
    shutil.copyfile(archive, docs / archive.name)
    return {"private": True, "version": lock["version"], "source": archive.name, "python_minor": f"{os.sys.version_info.major}.{os.sys.version_info.minor}"}


def headset_candidate(rootfs, target, work, artifacts, cache):
    require_build_user()
    system = shutil.which("headsetcontrol")
    if system:
        verified = probe_headset(system)
        if verified["configured"]:
            return {**verified, "selection": "system", "private": False}
    lock = json_file(HERE / "headsetcontrol.lock.json")
    data = fetch_locked(lock["source"], cache, f"headsetcontrol-{lock['version']}-source.tar.gz")
    source = extract_source(data, Path(work) / "headsetcontrol-source")
    archive = Path(artifacts) / f"headsetcontrol-{lock['version']}-source.tar.gz"
    archive.write_bytes(data)
    binary = Path(work) / "headsetcontrol-build/headsetcontrol"
    result = {"configured": False, "private": False, "version": lock["version"], "source": archive.name, "selection": "none"}
    try:
        if target["headset_build"] == "source":
            run(["cmake", "-S", str(source), "-B", str(binary.parent), "-DCMAKE_BUILD_TYPE=Release", f"-DHEADSETCONTROL_VERSION={lock['version']}", "-DBUILD_SHARED_LIBRARY=OFF"], capture=False, timeout=120)
            run(["cmake", "--build", str(binary.parent), "--parallel", str(os.cpu_count() or 2)], capture=False, timeout=600)
            run(["ctest", "--test-dir", str(binary.parent), "--output-on-failure"], capture=False, timeout=120)
        else:
            zipped = fetch_locked(lock["linux_x86_64"], cache, "headsetcontrol-linux-x86_64.zip")
            with zipfile.ZipFile(io.BytesIO(zipped)) as zipped_source:
                files = zipped_source.infolist()
                if len(files) != 1 or files[0].filename != "headsetcontrol" or files[0].file_size > 16 * 1024 * 1024:
                    raise PackagingError("Invalid pinned HeadsetControl binary archive")
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_bytes(zipped_source.read(files[0]))
                binary.chmod(0o755)
        from staging import elf_report
        elf_report(binary)
        verified = probe_headset(binary)
        result.update(verified)
        if verified["configured"]:
            destination = Path(rootfs) / "usr/lib/peripheral-hub/headsetcontrol"
            shutil.copyfile(binary, destination)
            destination.chmod(0o755)
            rules = run([str(binary), "-u"]).stdout
            # A private filename never overwrites the stock 70-headsets.rules.
            udev = Path(rootfs) / "usr/lib/udev/rules.d/70-peripheral-hub-headsets.rules"
            udev.parent.mkdir(parents=True, exist_ok=True)
            udev.write_text(rules, encoding="utf-8")
            docs = Path(rootfs) / "usr/share/doc/peripheral-hub/headsetcontrol"
            docs.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / "LICENSE", docs / "LICENSE")
            shutil.copyfile(archive, docs / archive.name)
            result.update(private=True, selection="private", path="/usr/lib/peripheral-hub/headsetcontrol")
    except PackagingError as error:
        result["reason"] = f"Pinned candidate is not target-compatible: {error}"
    if not result["configured"]:
        result["reason"] = "HeadsetControl component not configured: " + result.get("reason", "No candidate passes unchanged Rust API 1.8/1.9")
        result.pop("path", None)
    return result


def source_archive(destination, entries, epoch):
    destination = Path(destination)
    with destination.open("wb") as output:
        with gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for name, source in sorted(entries.items()):
                    safe_relative(name)
                    contents = regular_bytes(source, 2 ** 31)
                    info = tarfile.TarInfo(name)
                    info.size = len(contents)
                    info.uid = info.gid = 0
                    info.uname = info.gname = "root"
                    info.mtime = epoch
                    info.mode = 0o755 if Path(source).suffix == ".sh" else 0o644
                    archive.addfile(info, io.BytesIO(contents))
    return digest(destination.read_bytes())
