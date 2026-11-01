#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Non-root Linux build: Tauri payload + agent + private engine -> native bundle."""

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path

from common import (
    APP, HERE, PackagingError, digest, json_file, load_targets, package_name,
    parse_os_release, regular_bytes, require_build_user, resolve_packages,
    run, target_for, write_json,
)
from sources import evdev_private, headset_candidate, source_archive
from staging import audit, extract_tauri_deb, merge_rootfs, rootfs_archive

REPO = HERE.parent


def app_version(repo=REPO):
    npm = json_file(repo / "package.json")["version"]
    tauri = json_file(repo / "src-tauri/tauri.conf.json")["version"]
    cargo = re.search(r'^version\s*=\s*"([^"]+)"', regular_bytes(repo / "src-tauri/Cargo.toml").decode(), re.M)
    if not cargo or cargo[1] != npm or npm != tauri or npm != "0.1.0":
        raise PackagingError("package.json, Cargo.toml and Tauri must agree on the approved version 0.1.0")
    return npm


def native_requirements(target):
    requirements = target["runtime"] + target["headset"] + target.get("audio", []) + target.get("ratbag", [])
    # RPM OpenRazer requires an additional upstream repository; it is not a core dependency.
    if target["manager"] in ("apt", "pacman"):
        requirements += target.get("razer", [])
    if target["format"] == "deb":
        requirements += ["init-system-helpers"]
    resolved, missing = resolve_packages(target, requirements)
    if missing:
        raise PackagingError("Runtime dependencies unavailable in native metadata: " + ", ".join(missing))
    requirements = []
    for entry in resolved:
        old = package_name(entry["requirement"])
        requirements.append(entry["name"] + entry["requirement"][len(old):])
    return list(dict.fromkeys(requirements)), resolved


def package_deb(rootfs, reports, dependencies, work, destination, version):
    debian = Path(work) / "debian"
    debian.mkdir()
    (debian / "control").write_text(f"Source: {APP}\nSection: utils\nPriority: optional\nMaintainer: Peripheral Hub contributors <shadyMUI@users.noreply.github.com>\n\nPackage: {APP}\nArchitecture: amd64\nDescription: Peripheral Hub and private remapping engine\n", encoding="utf-8")
    command = ["dpkg-shlibdeps", "-O", *["-e" + str(Path(rootfs) / name) for name in reports]]
    shlibs = run(command, cwd=work).stdout.strip()
    if not shlibs.startswith("shlibs:Depends="):
        raise PackagingError("dpkg-shlibdeps did not provide actual ELF dependencies")
    depends = list(dict.fromkeys([*dependencies, *shlibs.split("=", 1)[1].split(", ")]))
    control = Path(rootfs) / "DEBIAN"
    control.mkdir()
    size = sum(path.stat().st_size for path in Path(rootfs).rglob("*") if path.is_file()) // 1024
    (control / "control").write_text(
        f"Package: {APP}\nVersion: {version}-1\nArchitecture: amd64\nSection: utils\nPriority: optional\n"
        f"Maintainer: Peripheral Hub contributors <shadyMUI@users.noreply.github.com>\nInstalled-Size: {size}\n"
        f"Depends: {', '.join(depends)}\nRecommends: pipewire-pulse\nHomepage: https://github.com/shadyMUI/sycomopse\n"
        "Description: Peripheral Hub GUI, user agent and private Input Remapper engine\n"
        " Native hooks activate the private backend; the desktop session starts the user agent.\n"
        " Separate GPL engine sources and licenses are included.\n", encoding="utf-8",
    )
    for name in ("postinst", "prerm", "postrm"):
        shutil.copyfile(HERE / "deb" / name, control / name)
        (control / name).chmod(0o755)
    (control / "conffiles").write_text("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop\n", encoding="utf-8")
    destination = Path(destination) / f"{APP}_{version}-1_amd64.deb"
    run(["dpkg-deb", "--root-owner-group", "--build", str(rootfs), str(destination)], capture=False, timeout=300)
    shutil.rmtree(control)
    return destination, run(["dpkg-deb", "--field", str(destination), "Depends"]).stdout.strip().split(", ")


def package_arch(rootfs_tar, reports, dependencies, work, destination, version):
    arch = Path(work) / "arch"
    arch.mkdir()
    for name in ("PKGBUILD", "peripheral-hub.install"):
        shutil.copyfile(HERE / "arch" / name, arch / name)
    owners = {}
    for report in reports.values():
        for library in report["resolved_libraries"]:
            owner = run(["pacman", "-Qqo", library]).stdout.strip()
            actual = run(["pacman", "-Q", owner]).stdout.strip().split()
            if len(actual) != 2:
                raise PackagingError("Cannot resolve ELF library owner/version via pacman")
            owners[actual[0]] = actual[1]
    dependencies = list(dict.fromkeys([*dependencies, *[f"{name}>={version}" for name, version in sorted(owners.items())]]))
    shutil.copyfile(rootfs_tar, arch / "rootfs.tar.gz")
    deps_file = arch / "native-depends"
    deps_file.write_text("\n".join(dependencies) + "\n", encoding="utf-8")
    env = dict(os.environ, PH_VERSION=version, PH_ROOTFS_SHA256=digest(regular_bytes(rootfs_tar, 2 ** 31)), PH_DEPENDS_SHA256=digest(deps_file.read_bytes()), PKGDEST=str(Path(destination).resolve()))
    run(["makepkg", "--cleanbuild", "--nodeps", "--noconfirm"], cwd=arch, env=env, capture=False, timeout=600)
    packages = list(Path(destination).glob(f"{APP}-{version}-1-x86_64.pkg.tar.*"))
    if len(packages) != 1:
        raise PackagingError("makepkg did not produce exactly one native package")
    return packages[0], dependencies


def package_rpm(rootfs_tar, rootfs, dependencies, work, destination, version):
    top = Path(work) / "rpmbuild"
    for name in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
        (top / name).mkdir(parents=True)
    shutil.copyfile(rootfs_tar, top / "SOURCES/rootfs.tar.gz")
    requires = top / "SOURCES/native-requires.inc"
    requires.write_text("\n".join("Requires: " + dependency for dependency in dependencies) + "\n", encoding="utf-8")
    filelist = top / "SOURCES/files.list"
    files = ["%defattr(-,root,root,-)"]
    for path in sorted(Path(rootfs).rglob("*")):
        if not path.is_file():
            continue
        name = "/" + path.relative_to(rootfs).as_posix()
        if '"' in name or "%" in name or "\n" in name:
            raise PackagingError("Unsupported RPM payload path")
        files.append(('%config(noreplace) ' if name.startswith("/etc/") else '') + '"' + name + '"')
    # Private directories must be owned by this package as well as their files.
    for path in sorted(Path(rootfs).rglob("*")):
        if path.is_dir() and path.relative_to(rootfs).as_posix().startswith(("usr/lib/peripheral-hub", "usr/share/doc/peripheral-hub")):
            files.append('%dir "/' + path.relative_to(rootfs).as_posix() + '"')
    filelist.write_text("\n".join(files) + "\n", encoding="utf-8")
    run(["rpmbuild", "-bb", "--define", f"_topdir {top}", "--define", f"ph_version {version}", "--define", f"ph_requires {requires}", "--define", f"ph_filelist {filelist}", str(HERE / "rpm/peripheral-hub.spec")], capture=False, timeout=600)
    packages = list((top / "RPMS").rglob(f"{APP}-{version}-1*.rpm"))
    if len(packages) != 1:
        raise PackagingError("rpmbuild did not produce exactly one native package")
    destination = Path(destination) / packages[0].name
    shutil.copyfile(packages[0], destination)
    return destination, run(["rpm", "-qp", "--requires", str(destination)]).stdout.strip().splitlines()


def app_sources(artifacts, epoch):
    entries = {}
    for name in ("src", "src-tauri", "packaging", ".github/workflows"):
        root = REPO / name
        if not root.is_dir():
            continue
        for path in root.rglob("*"):
            if not path.is_file() or path.is_symlink():
                continue
            relative = path.relative_to(REPO)
            if any(part in ("target", "node_modules", "out", ".cache", "__pycache__", ".git") for part in relative.parts) or path.suffix == ".pyc":
                continue
            if path.name.startswith(".env") or path.suffix in (".key", ".pem", ".p12", ".pfx"):
                continue
            entries["peripheral-hub-0.1.0/" + relative.as_posix()] = path
    for name in ("LICENSE", "README.md", "package.json", "package-lock.json", "tsconfig.json", "vite.config.ts", "index.html"):
        if (REPO / name).is_file():
            entries["peripheral-hub-0.1.0/" + name] = REPO / name
    path = Path(artifacts) / "peripheral-hub-0.1.0-corresponding-source.tar.gz"
    source_archive(path, entries, epoch)
    return path


def write_manifest(bundle, manifest):
    bundle = Path(bundle)
    manifest["files"] = {}
    for path in sorted(bundle.rglob("*")):
        if path.is_file() and path.name not in ("manifest.json", "manifest.sha256"):
            data = regular_bytes(path, 2 ** 31)
            manifest["files"][path.relative_to(bundle).as_posix()] = {"size": len(data), "sha256": digest(data)}
    write_json(bundle / "manifest.json", manifest)
    (bundle / "manifest.sha256").write_text(digest((bundle / "manifest.json").read_bytes()) + "  manifest.json\n", encoding="ascii")


def build(args):
    require_build_user()
    version = app_version()
    release = parse_os_release(regular_bytes("/etc/os-release", 65536).decode())
    target = target_for(release)
    if target["id"] != args.target:
        raise PackagingError("A native target must be built inside its own distro/version image")
    image_digest = args.image_digest or os.environ.get("PH_BUILD_IMAGE_DIGEST", "")
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", image_digest):
        raise PackagingError("Record the actual pulled image digest (--image-digest); no guessed or unverified target claim")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    bundle = output / f"peripheral-hub-{version}-{target['id']}-x86_64"
    if bundle.exists():
        raise PackagingError(f"Output bundle already exists; choose a new output path: {bundle}")
    bundle.mkdir()
    artifacts = bundle / "sources"
    artifacts.mkdir()
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0")) or int(run(["git", "show", "-s", "--format=%ct", "HEAD"], cwd=REPO).stdout.strip())
    cache = HERE / ".cache"
    with tempfile.TemporaryDirectory(prefix=".build-", dir=output) as temporary:
        work = Path(temporary)
        rootfs = work / "rootfs"
        rootfs.mkdir()
        env = dict(os.environ, CARGO_TARGET_DIR=str(work / "cargo"), SOURCE_DATE_EPOCH=str(epoch))
        run(["npm", "ci"], cwd=REPO, capture=False, timeout=600)
        run(["npm", "test"], cwd=REPO, capture=False, timeout=300)
        overlay = work / "tauri-native.json"
        write_json(overlay, {"bundle": {"active": True, "targets": ["deb"]}})
        run(["npm", "run", "tauri", "--", "build", "--bundles", "deb", "--config", str(overlay)], cwd=REPO, env=env, capture=False, timeout=3600)
        tauri_packages = list((work / "cargo/release/bundle/deb").glob("*.deb"))
        if len(tauri_packages) != 1:
            raise PackagingError("Tauri GUI baseline package was not produced")
        extract_tauri_deb(tauri_packages[0], rootfs)
        run(["cargo", "test", "--locked", "--manifest-path", str(REPO / "src-tauri/Cargo.toml"), "--no-default-features"], cwd=REPO, env=env, capture=False, timeout=600)
        run(["cargo", "build", "--locked", "--release", "--manifest-path", str(REPO / "src-tauri/Cargo.toml"), "--bin", "peripheral-hub-remapping-agent", "--no-default-features"], cwd=REPO, env=env, capture=False, timeout=1200)
        shutil.copyfile(work / "cargo/release/peripheral-hub-remapping-agent", rootfs / "usr/bin/peripheral-hub-remapping-agent")
        (rootfs / "usr/bin/peripheral-hub-remapping-agent").chmod(0o755)
        merge_rootfs(HERE / "rootfs", rootfs)
        merge_rootfs(HERE / "remapper/rootfs", rootfs)
        run(["python3", str(HERE / "remapper/fetch-engine.py"), "--destination", str(rootfs), "--source-archive", str(artifacts / "input-remapper-2.2.1-corresponding-source.tar.gz")], cwd=REPO, capture=False, timeout=180)
        shutil.copyfile(artifacts / "input-remapper-2.2.1-corresponding-source.tar.gz", rootfs / "usr/share/doc/peripheral-hub/remapper/input-remapper-2.2.1-corresponding-source.tar.gz")
        private_evdev = evdev_private(rootfs, work, artifacts, cache)
        headset = headset_candidate(rootfs, target, work, artifacts, cache)
        if not headset.get("configured") or not headset.get("private"):
            raise PackagingError("Native package requires a staged HeadsetControl CLI with verified JSON API 1.6: " + headset.get("reason", "probe failed"))
        docs = rootfs / "usr/share/doc/peripheral-hub"
        shutil.copyfile(REPO / "LICENSE", docs / "LICENSE.app")
        installer = rootfs / "usr/lib/peripheral-hub/installer"
        installer.mkdir(parents=True)
        for name in ("install.py", "system.py", "common.py", "native.py", "targets.json"):
            shutil.copyfile(HERE / name, installer / name)
            shutil.copyfile(HERE / name, bundle / name)
        launcher = bundle / "install.sh"
        shutil.copyfile(HERE / "install.sh", launcher)
        launcher.chmod(0o755)
        # The wrapper intentionally uses installed absolute paths. Check the staged bridge directly.
        run(["python3", "-I", "-B", str(rootfs / "usr/lib/peripheral-hub/remapper/bridge.py"), "--check-imports"], capture=False, timeout=30)
        for path in rootfs.rglob("*.desktop"):
            run(["desktop-file-validate", str(path)])
        reports = audit(rootfs)
        dependencies, resolved = native_requirements(target)
        if private_evdev["private"]:
            minor = private_evdev["python_minor"]
            if target["format"] == "deb":
                dependencies.append(f"python{minor}")
            elif target["format"] == "rpm":
                dependencies.append(f"python(abi) = {minor}")
            else:
                major, feature = (int(part) for part in minor.split("."))
                dependencies.extend([f"python>={minor}", f"python<{major}.{feature + 1}"])
        archive = work / "rootfs.tar.gz"
        rootfs_archive(rootfs, archive, epoch)
        if target["format"] == "deb":
            package, native = package_deb(rootfs, reports, dependencies, work, bundle, version)
        elif target["format"] == "arch":
            package, native = package_arch(archive, reports, dependencies, work, bundle, version)
        else:
            package, native = package_rpm(archive, rootfs, dependencies, work, bundle, version)
        # Keep a verifiable staged rootfs for integration audits, in addition to the native payload.
        shutil.copyfile(archive, bundle / "rootfs.tar.gz")
        app_sources(artifacts, epoch)
        sources = [{"file": "sources/" + path.name, "license": "MIT AND GPL-3.0-or-later AND BSD-3-Clause" if path.name.startswith("peripheral-hub-") else "BSD-3-Clause" if "evdev" in path.name else "GPL-3.0-or-later"} for path in sorted(artifacts.iterdir())]
        manifest = {
            "schema": 1, "app": {"name": APP, "version": version}, "architecture": "x86_64", "init": "systemd",
            "target": {"id": target["id"], "os_release": release, "image": target["image"], "image_digest": image_digest, "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "source_date_epoch": epoch},
            "package": {"file": package.name, "format": target["format"], "version": version + "-1", "signed": False},
            "requirements": {"elf": reports, "native": native, "resolved_packages": resolved, "python": ".".join(map(str, sys.version_info[:3]))},
            "toolchain": {"rustc": run(["rustc", "--version"]).stdout.strip(), "cargo": run(["cargo", "--version"]).stdout.strip(), "node": run(["node", "--version"]).stdout.strip()},
            "source_revision": run(["git", "rev-parse", "HEAD"], cwd=REPO).stdout.strip(),
            "source_tree_dirty": bool(run(["git", "status", "--porcelain"], cwd=REPO).stdout.strip()),
            "components": {"gui": "usr/bin/peripheral-hub", "agent": {"binary": "usr/bin/peripheral-hub-remapping-agent", "cargo_features": "--no-default-features"}, "input_remapper": {"version": "2.2.1", "commit": "e9a87d13480c3b1dee654b296578a8f4e2cd31d6", "bus": "io.peripheralhub.InputRemapper"}, "evdev": private_evdev, "headsetcontrol": headset},
            "sources": sources, "validation": {"elf": "passed", "python_imports": "passed", "native_smoke": "not-run", "linux_hardware": "not-run", "graphical_login_reboot": "not-run"},
        }
        write_manifest(bundle, manifest)
    print(f"Built native bundle: {bundle}; installation remains gated until smoke tests pass")
    return bundle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=[item["id"] for item in load_targets()["targets"]])
    parser.add_argument("--output", type=Path, default=HERE / "out")
    parser.add_argument("--image-digest")
    args = parser.parse_args()
    try:
        build(args)
    except (PackagingError, OSError, ValueError) as error:
        print(f"Build failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
