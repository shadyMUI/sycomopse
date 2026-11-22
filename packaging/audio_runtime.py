#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Pinned SOFA-capable PipeWire client, isolated from the server's private ABI."""

import argparse
import shutil
import sys
from pathlib import Path

from common import HERE, PackagingError, digest, json_file, regular_bytes, require_build_user, run, safe_relative, write_json
from sources import extract_source, fetch_locked

PREFIX = Path("/usr/lib/peripheral-hub/pipewire")
PRIVATE_LIB = "libperipheral-hub-pipewire-0.3.so.0"
PRIVATE_PROTOCOL = "libperipheral-hub-pipewire-protocol-native.so"
MODULES = ("protocol-native", "client-node", "client-device", "adapter", "filter-chain", "filter-chain-sofa")
BUILD_FILES = {
    "src/daemon/pipewire": "bin/pipewire",
    "src/pipewire/libpipewire-0.3.so.0.1005.0": "lib/" + PRIVATE_LIB,
    **{"src/modules/libpipewire-module-" + name + ".so": "lib/pipewire-0.3/libpipewire-module-" + name + ".so" for name in MODULES},
    "spa/plugins/support/libspa-support.so": "lib/spa-0.2/support/libspa-support.so",
    "spa/plugins/audioconvert/libspa-audioconvert.so": "lib/spa-0.2/audioconvert/libspa-audioconvert.so",
}


def meson_options():
    # Explicitly disable all optional integrations. No daemon/service/global
    # configuration is installed; the caller supplies its client-only graph.
    return ["--buildtype=release", "--wrap-mode=nodownload", "--prefix=" + str(PREFIX),
            "--libdir=lib", "-Dauto_features=disabled", "-Dlibmysofa=enabled", "-Dsndfile=enabled",
            "-Dsupport=enabled", "-Daudioconvert=enabled", "-Dspa-plugins=enabled",
            "-Ddbus=disabled", "-Dflatpak=disabled", "-Dpipewire-jack=disabled",
            "-Dpipewire-v4l2=disabled", "-Dexamples=disabled", "-Dtests=disabled",
            "-Dsession-managers=[]", "-Dsystemd-user-service=disabled", "-Drlimits-install=false"]


def rpath_for(relative):
    relative = Path(relative)
    if relative.parent == Path("bin"):
        return "$ORIGIN/../lib"
    if relative.parent == Path("lib"):
        return "$ORIGIN"
    if relative.parent == Path("lib/pipewire-0.3"):
        return "$ORIGIN:$ORIGIN/.."
    return "$ORIGIN/../.."


def pipewire_private(rootfs, work, artifacts, cache):
    require_build_user()
    rootfs, work, artifacts = Path(rootfs), Path(work), Path(artifacts)
    artifacts.mkdir(parents=True, exist_ok=True)
    lock = json_file(HERE / "pipewire.lock.json")
    if lock["version"] != "1.0.5" or lock["patchset"] != "1ubuntu3.3":
        raise PackagingError("Private PipeWire client needs a reviewed source/ABI generation")
    source_name = "pipewire-1.0.5-upstream.tar.bz2"
    debian_name = "pipewire-1.0.5-1ubuntu3.3-debian.tar.xz"
    source_data = fetch_locked(lock["source"], cache, source_name)
    debian_data = fetch_locked(lock["debian"], cache, debian_name)
    if len(source_data) != lock["source"]["size"] or len(debian_data) != lock["debian"]["size"]:
        raise PackagingError("Pinned PipeWire archive size mismatch")
    source = extract_source(source_data, work / "pipewire-source", allow_colons=True)
    debian = extract_source(debian_data, source / "debian")
    for line in regular_bytes(debian / "patches/series", 65536).decode().splitlines():
        name = line.split("#", 1)[0].strip()
        if not name:
            continue
        safe_relative(name)
        patch = debian / "patches" / name
        regular_bytes(patch, 1024 * 1024)
        run(["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i", str(patch)], cwd=source, timeout=30)
    build = work / "pipewire-build"
    run(["meson", "setup", str(build), str(source), *meson_options()], capture=False, timeout=180)
    run(["ninja", "-C", str(build), "-j", "2", *BUILD_FILES], capture=False, timeout=600)
    destination = rootfs / PREFIX.relative_to("/")
    if destination.exists():
        raise PackagingError("Private PipeWire destination already exists; never overwrite an existing generation")
    destination.mkdir(parents=True)
    for original, relative in BUILD_FILES.items():
        contents = regular_bytes(build / original, 32 * 1024 * 1024)
        if not contents.startswith(b"\x7fELF"):
            raise PackagingError("Missing private PipeWire ELF: " + original)
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(contents)
        target.chmod(0o755)
        run(["patchelf", "--set-rpath", rpath_for(relative), str(target)])
        needed = run(["patchelf", "--print-needed", str(target)]).stdout.splitlines()
        for old, new in (("libpipewire-0.3.so.0", PRIVATE_LIB),
                         ("libpipewire-module-protocol-native.so", PRIVATE_PROTOCOL)):
            if old in needed:
                run(["patchelf", "--replace-needed", old, new, str(target)])
    # Unique SONAMEs prevent the dynamic loader from silently falling back to
    # host libpipewire/module internals if a private dependency goes missing.
    run(["patchelf", "--set-soname", PRIVATE_LIB, str(destination / "lib" / PRIVATE_LIB)])
    protocol = destination / "lib/pipewire-0.3/libpipewire-module-protocol-native.so"
    run(["patchelf", "--set-soname", PRIVATE_PROTOCOL, str(protocol)])
    shutil.copyfile(protocol, protocol.with_name(PRIVATE_PROTOCOL))
    protocol.with_name(PRIVATE_PROTOCOL).chmod(0o755)
    from staging import elf_report
    reports = {}
    for path in sorted(destination.rglob("*")):
        if path.is_file():
            report = elf_report(path)
            if any(library in report["needed"] for library in ("libpipewire-0.3.so.0", "libpipewire-module-protocol-native.so")):
                raise PackagingError("Private client still links host PipeWire internals")
            reports[path.relative_to(destination).as_posix()] = report
    version = run([str(destination / "bin/pipewire"), "--version"], timeout=10).stdout
    if "Compiled with libpipewire 1.0.5" not in version or "Linked with libpipewire 1.0.5" not in version:
        raise PackagingError("Private PipeWire executable and library versions are incoherent")
    docs = rootfs / "usr/share/doc/peripheral-hub/pipewire"
    docs.mkdir(parents=True, exist_ok=True)
    for name, data in ((source_name, source_data), (debian_name, debian_data)):
        (artifacts / name).write_bytes(data)
        (docs / name).write_bytes(data)
    shutil.copyfile(source / "COPYING", docs / "COPYING")
    shutil.copyfile(debian / "copyright", docs / "copyright.debian")
    if (source / "LICENSES").is_dir():
        shutil.copytree(source / "LICENSES", docs / "LICENSES")
    shutil.copyfile(HERE / "pipewire.lock.json", docs / "pipewire.lock.json")
    shutil.copyfile(HERE / "audio_runtime.py", docs / "audio_runtime.py")
    result = {"schemaVersion": 1, "kind": "private-pipewire-client", "generation": lock["generation"],
              "version": lock["version"], "patchset": lock["patchset"], "nativeProtocol": lock["native_protocol"],
              "prefix": str(PREFIX), "binary": str(PREFIX / "bin/pipewire"),
              "moduleDir": str(PREFIX / "lib/pipewire-0.3"), "spaPluginDir": str(PREFIX / "lib/spa-0.2"),
              "sourceSha256": lock["source"]["sha256"], "patchsetSha256": lock["debian"]["sha256"],
              "files": {name: digest(regular_bytes(destination / name, 32 * 1024 * 1024)) for name in reports},
              "license": lock["license"], "private": True, "compiled": True}
    write_json(destination / "runtime.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rootfs", required=True, type=Path)
    parser.add_argument("--workdir", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--cache", required=True, type=Path)
    args = parser.parse_args()
    print(pipewire_private(args.rootfs, args.workdir, args.artifacts, args.cache))


if __name__ == "__main__":
    try:
        main()
    except (PackagingError, OSError, ValueError) as error:
        print("Private audio runtime build failed: " + str(error), file=sys.stderr)
        sys.exit(1)
