#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Resolve build/runtime packages using the container's signed native metadata."""

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import ENGINE, PackagingError, load_targets, resolve_packages, run


def allow_ubuntu_smoke_autostart(policy):
    # Adapt only the Ubuntu OCI image's stock deny-all policy, not custom policies.
    if policy.is_symlink() or not policy.is_file() or policy.read_bytes() != b"#!/bin/sh\nexit 101\n":
        return
    policy.write_bytes(f'#!/bin/sh\ncase "$1" in\n    {ENGINE}) exit 0 ;;\nesac\nexit 101\n'.encode("ascii"))
    print(f"Ubuntu smoke image: allowing only {ENGINE} through policy-rc.d", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--runtime-only", action="store_true")
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() != 0 or os.environ.get("PH_CI_CONTAINER") != "1" or not any(Path(name).is_file() for name in ("/.dockerenv", "/run/.containerenv", "/.containerenv")):
        raise PackagingError("Native dependency installation is allowed only in the explicit CI container")
    target = next(item for item in load_targets()["targets"] if item["id"] == args.target)
    audio_sdk = {
        "apt": ["libpipewire-0.3-dev", "libspa-0.2-dev", "libmysofa-dev", "lua5.4", "meson", "libsndfile1-dev", "patchelf"],
        "pacman": ["pipewire", "libmysofa", "lua", "meson", "libsndfile", "patchelf"],
        "dnf": ["pipewire-devel", "libmysofa-devel", "lua", "meson", "libsndfile-devel", "patchelf"],
        "zypper": ["pipewire-devel", "libmysofa-devel", "lua54", "meson", "libsndfile-devel", "patchelf"],
    }
    if args.runtime_only:
        # Fedora's minimal image needs the provider of runuser for the non-root smoke session.
        packages = target["runtime"] + (["util-linux"] if target["manager"] == "dnf" else [])
    else:
        packages = target["build"] + target["runtime"] + target["headset"] + target.get("audio", []) + audio_sdk[target["manager"]]
    resolved, missing = resolve_packages(target, packages)
    if missing:
        raise PackagingError("Clean image lacks required signed build packages: " + ", ".join(missing))
    names = list(dict.fromkeys(item["name"] for item in resolved))
    commands = {
        "apt": ["apt-get", "install", "--yes", "--no-install-recommends", *names],
        "pacman": ["pacman", "-S", "--needed", "--noconfirm", "--", *names],
        "dnf": ["dnf", "install", "--assumeyes", *names],
        "zypper": ["zypper", "--non-interactive", "install", *names],
    }
    run(commands[target["manager"]], capture=False, timeout=1800)
    if args.runtime_only and target["id"].startswith("ubuntu-"):
        allow_ubuntu_smoke_autostart(Path("/usr/sbin/policy-rc.d"))


if __name__ == "__main__":
    main()
