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
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() != 0 or os.environ.get("PH_CI_CONTAINER") != "1" or not Path("/.dockerenv").exists():
        raise PackagingError("Native dependency installation is allowed only in the explicit CI container")
    target = next(item for item in load_targets()["targets"] if item["id"] == args.target)
    packages = target["build"] + target["runtime"] + target["headset"]
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
