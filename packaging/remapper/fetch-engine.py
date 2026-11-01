#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Verify and stage the private engine, never pip-install or activate services."""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import urllib.request


HERE = Path(__file__).resolve().parent
MAX_ARCHIVE = 16 * 1024 * 1024
MAX_EXPANDED = 64 * 1024 * 1024
PRIVATE_DIR = Path("usr/lib/peripheral-hub/remapper")
DOC_DIR = Path("usr/share/doc/peripheral-hub/remapper")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def verified_members(data, lock):
    if sha256(data) != lock["archiveSha256"]:
        raise ValueError("Input Remapper source archive SHA256 mismatch")
    files = {}
    translation_aliases = {}
    expanded = 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            parts = PurePosixPath(member.name).parts
            if (
                not parts
                or parts[0] != lock["archiveRoot"]
                or any(part in (".", "..") or "\\" in part for part in parts)
                or PurePosixPath(member.name).is_absolute()
            ):
                raise ValueError("Unsafe path in engine archive")
            if member.isdir():
                continue
            relative = "/".join(parts[1:])
            if member.issym():
                target = PurePosixPath(member.linkname)
                if (
                    len(parts) != 3
                    or parts[1] != "po"
                    or len(target.parts) != 1
                    or target.is_absolute()
                    or target.parts[0] in (".", "..")
                    or "\\" in member.linkname
                    or not target.name.endswith(".po")
                    or relative in translation_aliases
                ):
                    raise ValueError("Only local upstream translation aliases are permitted")
                translation_aliases[relative] = "po/" + target.name
                continue
            if not member.isfile() or member.size < 0:
                raise ValueError("Links and special files are forbidden in engine archive")
            expanded += member.size
            if expanded > MAX_EXPANDED:
                raise ValueError("Engine archive exceeds expanded size limit")
            if not relative or relative in files or relative in translation_aliases:
                raise ValueError("Duplicate or invalid engine archive member")
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError("Unreadable engine archive member")
            contents = stream.read(member.size + 1)
            if len(contents) != member.size:
                raise ValueError("Truncated engine archive member")
            files[relative] = contents
    for alias, target in translation_aliases.items():
        if target not in files:
            raise ValueError("Translation alias target is missing")
        files[alias] = files[target]
    required = (
        "LICENSE",
        "pyproject.toml",
        "inputremapper/installation_info.py",
        "inputremapper/injection/injector.py",
    )
    if any(path not in files for path in required):
        raise ValueError("Source archive is missing the pinned engine")
    if b'VERSION = "2.2.1"' not in files["inputremapper/installation_info.py"]:
        raise ValueError("Source archive engine version does not match the lock")
    return files


def source_bundle(destination, files, lock, upstream_archive):
    """Ship exact upstream sources and every build/bridge source, without cache."""
    entries = {f"input-remapper-{lock['version']}/{path}": data for path, data in files.items()}
    entries[f"upstream/input-remapper-{lock['version']}-source.tar.gz"] = upstream_archive
    generated_docs = {
        (DOC_DIR / "LICENSE.input-remapper").as_posix(),
        (DOC_DIR / "source-lock.json").as_posix(),
    }
    for path in sorted(HERE.rglob("*")):
        relative = path.relative_to(HERE)
        if not path.is_file() or any(
            part.startswith(".cache")
            or part.startswith(".stage")
            or part in ("__pycache__", "vendor")
            for part in relative.parts
        ):
            continue
        if path.suffix == ".pyc" or relative.as_posix().endswith("-source.tar.gz") or relative.as_posix().removeprefix("rootfs/") in generated_docs:
            continue
        entries[f"peripheral-hub-remapper/{relative.as_posix()}"] = path.read_bytes()
    entries[f"peripheral-hub-remapper/rootfs/{DOC_DIR.as_posix()}/LICENSE.input-remapper"] = files["LICENSE"]
    entries[f"peripheral-hub-remapper/rootfs/{DOC_DIR.as_posix()}/source-lock.json"] = (HERE / "source-lock.json").read_bytes()
    entries["SOURCE-CHECKSUMS.sha256"] = "".join(f"{sha256(data)}  {name}\n" for name, data in sorted(entries.items())).encode("utf-8")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("wb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=lock["sourceDateEpoch"]) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for name, data in sorted(entries.items()):
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = 0o755 if name.endswith("/remapper-service") else 0o644
                    info.mtime = lock["sourceDateEpoch"]
                    info.uid = info.gid = 0
                    info.uname = info.gname = "root"
                    archive.addfile(info, io.BytesIO(data))
    return sha256(destination.read_bytes())


def stage(destination, data, lock):
    files = verified_members(data, lock)
    destination = destination.resolve()
    if destination == Path(destination.anchor) or destination == Path("/usr"):
        raise ValueError("Destination must be a rootfs staging directory, not the host root")
    destination.mkdir(parents=True, exist_ok=True)
    private = destination / PRIVATE_DIR
    private.mkdir(parents=True, exist_ok=True)
    vendor = private / "vendor"
    with tempfile.TemporaryDirectory(prefix=".engine-", dir=private) as temporary:
        staged = Path(temporary) / "vendor"
        staged.mkdir()
        manifest = {}
        for relative, contents in sorted(files.items()):
            if not relative.startswith("inputremapper/"):
                continue
            target = staged / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(contents)
            target.chmod(0o644)
            os.utime(target, (lock["sourceDateEpoch"], lock["sourceDateEpoch"]))
            manifest[relative] = sha256(contents)
        engine_lock = {
            "version": lock["version"],
            "commit": lock["commit"],
            "archiveSha256": lock["archiveSha256"],
            "files": manifest,
        }
        (staged / "engine-lock.json").write_text(json.dumps(engine_lock, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        if vendor.is_symlink():
            raise ValueError("Vendor staging directory must not be a symlink")
        vendor.mkdir(exist_ok=True)
        engine_directory = vendor / "inputremapper"
        if engine_directory.is_symlink():
            raise ValueError("Engine staging directory must not be a symlink")
        if engine_directory.exists():
            for path in engine_directory.rglob("*"):
                if path.is_symlink():
                    raise ValueError("Existing engine staging paths must not be symlinks")
                if path.is_file() and "inputremapper/" + path.relative_to(engine_directory).as_posix() not in manifest:
                    path.unlink()
        shutil.copytree(staged / "inputremapper", engine_directory, dirs_exist_ok=True)
        (staged / "engine-lock.json").replace(vendor / "engine-lock.json")
    docs = destination / DOC_DIR
    docs.mkdir(parents=True, exist_ok=True)
    (docs / "LICENSE.input-remapper").write_bytes(files["LICENSE"])
    (docs / "source-lock.json").write_bytes((HERE / "source-lock.json").read_bytes())
    (docs / f"input-remapper-{lock['version']}-source.tar.gz").write_bytes(data)
    wrapper = destination / "usr/lib/peripheral-hub/remapper-service"
    if wrapper.is_file():
        wrapper.chmod(0o755)
    return files, engine_lock


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", required=True, type=Path, help="Root of a build-only rootfs staging tree")
    parser.add_argument("--archive", type=Path, help="Use an already downloaded, checksum-verified source archive")
    parser.add_argument("--offline", action="store_true", help="Reject network fetching; requires --archive")
    parser.add_argument("--source-archive", type=Path, help="Write a reproducible corresponding GPL source bundle")
    args = parser.parse_args()
    if os.name == "posix" and os.geteuid() == 0:
        parser.error("Fetch/staging must run as a non-root build user")
    lock = json.loads((HERE / "source-lock.json").read_text(encoding="utf-8"))
    if args.archive:
        if args.archive.stat().st_size > MAX_ARCHIVE:
            parser.error("Source archive exceeds size limit")
        data = args.archive.read_bytes()
    else:
        if args.offline:
            parser.error("--offline requires --archive")
        request = urllib.request.Request(lock["archiveUrl"], headers={"User-Agent": "peripheral-hub-source-fetch/1"})
        with urllib.request.urlopen(request, timeout=60) as response:
            if not response.url.startswith("https://codeload.github.com/"):
                raise ValueError("Source download redirected outside pinned HTTPS origin")
            data = response.read(MAX_ARCHIVE + 1)
        if len(data) > MAX_ARCHIVE:
            raise ValueError("Source archive exceeds size limit")
    files, engine_lock = stage(args.destination, data, lock)
    result = {
        "version": engine_lock["version"],
        "commit": engine_lock["commit"],
        "archiveSha256": engine_lock["archiveSha256"],
        "destination": str(args.destination),
        "engineFiles": len(engine_lock["files"]),
    }
    if args.source_archive:
        result["correspondingSourceSha256"] = source_bundle(args.source_archive, files, lock, data)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
