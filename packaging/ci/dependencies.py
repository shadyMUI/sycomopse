#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Resolve build/runtime packages using the container's signed native metadata."""

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import PackagingError, load_targets, resolve_packages, run


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
    packages = target["runtime"] if args.runtime_only else target["build"] + target["runtime"] + target["headset"] + target.get("audio", []) + audio_sdk[target["manager"]]
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


if __name__ == "__main__":
    main()
