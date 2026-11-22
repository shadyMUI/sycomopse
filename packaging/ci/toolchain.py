#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fetch versioned official toolchains as a non-root CI user, never curl-pipe."""

import argparse
import hashlib
import os
import posixpath
import re
import shutil
import sys
import tarfile
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import PackagingError, https_bytes, require_build_user, run, safe_relative


def extract_node(archive, destination, expected_root):
    links = []
    seen = set()
    expanded = 0
    with tarfile.open(archive, "r:xz") as source:
        for member in source:
            relative = safe_relative(member.name.rstrip("/"))
            if relative.parts[0] != expected_root or str(relative) in seen:
                raise PackagingError("Invalid/duplicate official toolchain archive path")
            seen.add(str(relative))
            target = Path(destination).joinpath(*relative.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                expanded += member.size
                if member.size < 0 or expanded > 512 * 1024 * 1024:
                    raise PackagingError("Toolchain archive exceeds expanded size limit")
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as contents, target.open("xb") as output:
                    shutil.copyfileobj(contents, output)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)
            elif member.issym():
                linked = posixpath.normpath(posixpath.join(str(relative.parent), member.linkname))
                link_target = safe_relative(linked)
                if link_target.parts[0] != expected_root or member.linkname.startswith("/"):
                    raise PackagingError("Toolchain symlink escapes its own tree")
                links.append((target, member.linkname))
            else:
                raise PackagingError("Toolchain hardlinks/special files are forbidden")
    # Materialize bounded internal links last, so no archive member can write through them.
    for target, linked in links:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.symlink_to(linked)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True, type=Path)
    args = parser.parse_args()
    require_build_user()
    if os.environ.get("PH_CI_CONTAINER") != "1" or not any(Path(name).is_file() for name in ("/.dockerenv", "/run/.containerenv", "/.containerenv")):
        raise PackagingError("Toolchain preparation is restricted to CI containers")
    prefix = args.prefix.resolve()
    if not prefix.is_dir() or prefix.stat().st_uid != os.geteuid():
        raise PackagingError("Toolchain prefix must already exist and belong to the non-root builder")
    node_version = "24.11.0"
    node_file = f"node-v{node_version}-linux-x64.tar.xz"
    base = f"https://nodejs.org/dist/v{node_version}/"
    checksums = https_bytes(base + "SHASUMS256.txt", 65536).decode("ascii")
    matches = [line.split()[0] for line in checksums.splitlines() if len(line.split()) == 2 and line.split()[1] == node_file]
    if len(matches) != 1 or not re.fullmatch(r"[a-f0-9]{64}", matches[0]):
        raise PackagingError("Official Node.js checksum must identify exactly one archive")
    expected = matches[0]
    node = https_bytes(base + node_file, 128 * 1024 * 1024)
    if hashlib.sha256(node).hexdigest() != expected:
        raise PackagingError("Official Node.js archive checksum mismatch")
    with tempfile.TemporaryDirectory(dir=prefix) as temporary:
        archive = Path(temporary) / node_file
        archive.write_bytes(node)
        extract_node(archive, temporary, f"node-v{node_version}-linux-x64")
        shutil.move(str(Path(temporary) / f"node-v{node_version}-linux-x64"), prefix / "node")
    rustup_url = "https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init"
    binary = https_bytes(rustup_url, 32 * 1024 * 1024)
    checksum = https_bytes(rustup_url + ".sha256", 1024).decode("ascii").split()
    if not checksum:
        raise PackagingError("Official rustup installer checksum is missing")
    expected = checksum[0]
    if not re.fullmatch(r"[a-f0-9]{64}", expected) or hashlib.sha256(binary).hexdigest() != expected:
        raise PackagingError("Official rustup-init checksum mismatch")
    installer = prefix / "rustup-init"
    installer.write_bytes(binary)
    installer.chmod(0o755)
    env = dict(os.environ, CARGO_HOME=str(prefix / "cargo"), RUSTUP_HOME=str(prefix / "rustup"))
    run([str(installer), "-y", "--no-modify-path", "--profile", "minimal", "--component", "rustfmt", "--default-toolchain", "1.91.0"], env=env, capture=False, timeout=900)
    installer.unlink()


if __name__ == "__main__":
    main()
