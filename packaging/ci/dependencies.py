#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Resolve build/runtime packages using the container's signed native metadata."""

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import ENGINE, PackagingError, digest, load_targets, resolve_packages, run, version_at_least


_STOCK_APT_POLICY_SHA256 = {
    "c2bcd9decf63ff2c0d9f473f38bc3607900530aad80f99139855d56678456230",  # Ubuntu
    "21a8dca0d6a52cbac52cb7a86178d973616ffb42ea08747e6ff311659ab095e1",  # Debian
}


def allow_apt_smoke_autostart(policy):
    # Adapt only the official OCI images' stock deny-all policies, not custom policies.
    if policy.is_symlink() or not policy.is_file() or digest(policy.read_bytes()) not in _STOCK_APT_POLICY_SHA256:
        return
    policy.write_bytes(f'#!/bin/sh\ncase "$1" in\n    {ENGINE}) exit 0 ;;\nesac\nexit 101\n'.encode("ascii"))
    print(f"APT smoke image: allowing only {ENGINE} through policy-rc.d", flush=True)


def prepare_trixie_pipewire(runtime_only, sources_dir=Path("/etc/apt/sources.list.d")):
    # Stock 1.4.2 misses Props notifications on an unnegotiated mixer (PipeWire #4610).
    # Test the same supported runtime as the Debian 13 .deb, from Debian's signed backports.
    (sources_dir / "peripheral-hub-ci-backports.sources").write_text(
        "Types: deb\nURIs: https://deb.debian.org/debian\nSuites: trixie-backports\n"
        "Components: main\nSigned-By: /usr/share/keyrings/debian-archive-keyring.gpg\n",
        encoding="ascii",
    )
    stack = ["pipewire", "pipewire-bin", "libpipewire-0.3-0t64", "libpipewire-0.3-modules", "libspa-0.2-modules"]
    if not runtime_only:
        stack += ["libpipewire-0.3-dev", "libspa-0.2-dev"]
    run(["apt-get", "update"], capture=False, timeout=600)
    run(["apt-get", "install", "--yes", "--no-install-recommends", "--no-remove", "-t", "trixie-backports", *stack],
        capture=False, timeout=1800)
    installed = run(["dpkg-query", "-W", "-f=${Version}", "pipewire-bin"]).stdout.strip()
    if not version_at_least(installed, "1.4.10"):
        raise PackagingError(f"Debian 13 requires PipeWire >= 1.4.10 for confirmed mute readback, found {installed!r}")
    print(f"Debian 13 CI uses PipeWire {installed}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--runtime-only", action="store_true")
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() != 0 or os.environ.get("PH_CI_CONTAINER") != "1" or not any(Path(name).is_file() for name in ("/.dockerenv", "/run/.containerenv", "/.containerenv")):
        raise PackagingError("Native dependency installation is allowed only in the explicit CI container")
    target = next(item for item in load_targets()["targets"] if item["id"] == args.target)
    if target["id"] == "debian-13":
        prepare_trixie_pipewire(args.runtime_only)
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
        if target["id"] == "debian-12":
            # Bookworm's GCC 12 is too old for the pinned HeadsetControl release.
            packages += ["clang-19", "libc++-19-dev", "libc++abi-19-dev"]
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
    if args.runtime_only and target["manager"] == "apt":
        allow_apt_smoke_autostart(Path("/usr/sbin/policy-rc.d"))


if __name__ == "__main__":
    main()
