#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Non-root Linux build: Tauri payload + agent + private engine -> native bundle."""

import argparse
import copy
import ctypes
import json
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path

from audio_runtime import PRIVATE_LIB, PREFIX as AUDIO_PREFIX, pipewire_private
from common import (
    APP, HERE, PackagingError, digest, json_file, load_targets, package_name,
    parse_os_release, regular_bytes, require_build_user, resolve_packages,
    run, target_for, verify_bundle, write_json,
)
from sources import evdev_private, headset_candidate, source_archive
from staging import audit, extract_tauri_deb, merge_rootfs, rootfs_archive
from ci.gates import HELPER_BINARIES, require_native_preconditions, require_unpromoted, software_gate

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
    (debian / "control").write_text(f"Source: {APP}\nSection: utils\nPriority: optional\nMaintainer: Peripheral Hub contributors <shadyMUI@users.noreply.github.com>\n\nPackage: {APP}\nArchitecture: amd64\nDescription: Sycomopse and private remapping engine\n", encoding="utf-8")
    # Only the unique private core is self-provided. Do not ignore unknown
    # system libraries: mysofa/sndfile dependencies must come from real ELFs.
    (debian / "shlibs.local").write_text(f"{PRIVATE_LIB.split('.so.')[0]} 0 {APP}\n", encoding="ascii")
    private = Path(rootfs) / AUDIO_PREFIX.relative_to("/") / "lib"
    command = ["dpkg-shlibdeps", "-O", "-x" + APP, "-l" + str(private),
               "-l" + str(private / "pipewire-0.3"), *["-e" + str(Path(rootfs) / name) for name in reports]]
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
        "Description: Sycomopse GUI, user agent and private Input Remapper engine\n"
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


def package_arch(rootfs_tar, rootfs, reports, dependencies, work, destination, version):
    arch = Path(work) / "arch"
    arch.mkdir()
    for name in ("PKGBUILD", "peripheral-hub.install"):
        shutil.copyfile(HERE / "arch" / name, arch / name)
    owners = {}
    for report in reports.values():
        for library in report["resolved_libraries"]:
            if Path(library).resolve().is_relative_to(Path(rootfs).resolve()):
                continue
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
    """Create the initial inventory, never recertify an existing verified payload."""
    if "files" in manifest:
        raise PackagingError("Initial manifest writer cannot replace a verified inventory; use validation-only promotion")
    bundle = Path(bundle)
    manifest["files"] = {}
    for path in sorted(bundle.rglob("*")):
        if path.is_file() and path.name not in ("manifest.json", "manifest.sha256"):
            data = regular_bytes(path, 2 ** 31)
            manifest["files"][path.relative_to(bundle).as_posix()] = {"size": len(data), "sha256": digest(data)}
    write_json(bundle / "manifest.json", manifest)
    (bundle / "manifest.sha256").write_text(digest((bundle / "manifest.json").read_bytes()) + "  manifest.json\n", encoding="ascii")


def _cas_manifest(bundle, expected_sha, manifest):
    """Exchange the manifest atomically, then compare the displaced original."""
    if sys.platform != "linux":
        raise PackagingError("Native validation CAS requires Linux renameat2")
    data = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    proposed_sha = digest(data)
    descriptor, temporary = tempfile.mkstemp(prefix=".native-validation-", dir=Path(bundle).parent)
    temporary, destination = Path(temporary), Path(bundle) / "manifest.json"
    checksum_temp = None
    try:
        with os.fdopen(descriptor, "wb") as output:
            os.fchmod(output.fileno(), 0o644)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        exchange = ctypes.CDLL(None, use_errno=True).renameat2
        exchange.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        exchange.restype = ctypes.c_int
        def swap():
            if exchange(-100, os.fsencode(temporary), -100, os.fsencode(destination), 2):
                error = ctypes.get_errno()
                raise PackagingError("Atomic manifest CAS unavailable: " + os.strerror(error))
        swap()
        if digest(regular_bytes(temporary)) != expected_sha:
            if digest(regular_bytes(destination)) == proposed_sha:
                swap()
            raise PackagingError("Original manifest changed during atomic validation CAS")
        if digest(regular_bytes(destination)) != proposed_sha:
            raise PackagingError("Manifest changed after atomic validation CAS")
        checksum_fd, checksum_temp = tempfile.mkstemp(prefix=".native-checksum-", dir=Path(bundle).parent)
        with os.fdopen(checksum_fd, "wb") as output:
            os.fchmod(output.fileno(), 0o644)
            output.write((proposed_sha + "  manifest.json\n").encode("ascii"))
            output.flush()
            os.fsync(output.fileno())
        os.replace(checksum_temp, Path(bundle) / "manifest.sha256")
        return proposed_sha
    except AttributeError as error:
        raise PackagingError("Atomic manifest CAS unavailable; no unsafe replacement fallback") from error
    finally:
        temporary.unlink(missing_ok=True)
        if checksum_temp is not None:
            Path(checksum_temp).unlink(missing_ok=True)


def update_validation(bundle, expected_sha, validation):
    """CAS the manifest's validation only; payload and source/license hashes never change."""
    if not isinstance(expected_sha, str) or not re.fullmatch(r"[a-f0-9]{64}", expected_sha):
        raise PackagingError("Validation update requires the explicit original manifest SHA256")
    bundle = Path(bundle)
    manifest, _ = verify_bundle(bundle, expected_sha)
    require_native_preconditions(manifest)
    require_unpromoted(manifest)
    allowed = {"native_smoke", "native_smoke_steps", "native_smoke_evidence", "native_smoke_limitations",
               "linux_hardware", "graphical_login_reboot", "audio_hardware", "dkms_host_kernel"}
    if not isinstance(validation, dict) or set(validation) - allowed or validation.get("native_smoke") not in ("not-run", "passed"):
        raise PackagingError("Only native validation fields may be updated")
    candidate = copy.deepcopy(manifest)
    candidate["validation"].update(copy.deepcopy(validation))
    if validation["native_smoke"] == "passed":
        evidence = candidate["validation"].get("native_smoke_evidence", {})
        if evidence.get("input_manifest_sha256") != expected_sha:
            raise PackagingError("Native report is not bound to the original verified manifest")
        evidence["inventory_sha256"] = digest(json.dumps(manifest["files"], sort_keys=True, separators=(",", ":")).encode())
    else:
        candidate["validation"].pop("native_smoke_evidence", None)
        candidate["validation"]["native_smoke_steps"] = []
    # Cooperating attempts serialize; the atomic exchange also compares an
    # independently replaced original instead of silently overwriting its bytes.
    descriptor = os.open(bundle / "manifest.json", os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        if sys.platform == "linux":
            import fcntl
            fcntl.flock(descriptor, fcntl.LOCK_EX)
        current, _ = verify_bundle(bundle, expected_sha)
        if current != manifest or digest(regular_bytes(bundle / "manifest.json")) != expected_sha:
            raise PackagingError("Original manifest changed before validation CAS")
        promoted_sha = _cas_manifest(bundle, expected_sha, candidate)
        try:
            checked, _ = verify_bundle(bundle, promoted_sha)
            if checked != candidate or checked["files"] != manifest["files"]:
                raise PackagingError("Manifest or inventory changed during validation promotion")
        except BaseException:
            # Preserve the verified inventory even when the filesystem no longer
            # matches it; never hash the substituted payload into a successful report.
            if digest(regular_bytes(bundle / "manifest.json")) == promoted_sha:
                invalid = copy.deepcopy(candidate)
                invalid["validation"].update(native_smoke="not-run", native_smoke_steps=[])
                invalid["validation"].pop("native_smoke_evidence", None)
                _cas_manifest(bundle, promoted_sha, invalid)
            raise
        return checked, promoted_sha
    finally:
        os.close(descriptor)


def audit_binary_payload(rootfs):
    reports = audit(rootfs)
    for binary in HELPER_BINARIES:
        name = "usr/bin/" + binary
        if name not in reports:
            raise PackagingError("Mandatory headless executable is missing or not ELF: " + name)
        if any("gtk" in library.lower() or "webkit" in library.lower() for library in reports[name]["needed"]):
            raise PackagingError(binary + " linked desktop libraries; build must use --no-default-features")
    return reports


def build(args):
    require_build_user()
    version = app_version()
    release = parse_os_release(regular_bytes(Path("/etc/os-release").resolve(), 65536, owner=0).decode())
    target = target_for(release, allow_compatible=False)
    if target["id"] != args.target or release["ID"] != target["os_id"]:
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
        software = software_gate(work / "software-gate", work / "cargo", REPO)
        overlay = work / "tauri-native.json"
        write_json(overlay, {"bundle": {"active": True, "targets": ["deb"]}})
        run(["npm", "run", "tauri", "--", "build", "--bundles", "deb", "--config", str(overlay), "--", "--locked"], cwd=REPO, env=env, capture=False, timeout=3600)
        tauri_packages = list((work / "cargo/release/bundle/deb").glob("*.deb"))
        if len(tauri_packages) != 1:
            raise PackagingError("Tauri GUI baseline package was not produced")
        extract_tauri_deb(tauri_packages[0], rootfs)
        helper_args = [argument for binary in HELPER_BINARIES for argument in ("--bin", binary)]
        run(["cargo", "build", "--locked", "--release", "--manifest-path", str(REPO / "src-tauri/Cargo.toml"), "--no-default-features", *helper_args], cwd=REPO, env=env, capture=False, timeout=1200)
        for binary in HELPER_BINARIES:
            shutil.copyfile(work / "cargo/release" / binary, rootfs / "usr/bin" / binary)
            (rootfs / "usr/bin" / binary).chmod(0o755)
        merge_rootfs(HERE / "rootfs", rootfs)
        merge_rootfs(HERE / "remapper/rootfs", rootfs)
        run(["python3", str(HERE / "remapper/fetch-engine.py"), "--destination", str(rootfs), "--source-archive", str(artifacts / "input-remapper-2.2.1-corresponding-source.tar.gz")], cwd=REPO, capture=False, timeout=180)
        shutil.copyfile(artifacts / "input-remapper-2.2.1-corresponding-source.tar.gz", rootfs / "usr/share/doc/peripheral-hub/remapper/input-remapper-2.2.1-corresponding-source.tar.gz")
        private_evdev = evdev_private(rootfs, work, artifacts, cache)
        pipewire_client = pipewire_private(rootfs, work, artifacts, cache)
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
        write_json(installer / "build-target.json", {"schema": 1, "id": target["id"],
                   "os_release": release, "image": target["image"], "image_digest": image_digest})
        launcher = bundle / "install.sh"
        shutil.copyfile(HERE / "install.sh", launcher)
        launcher.chmod(0o755)
        # The wrapper intentionally uses installed absolute paths. Check the staged bridge directly.
        run(["python3", "-I", "-B", str(rootfs / "usr/lib/peripheral-hub/remapper/bridge.py"), "--check-imports"], capture=False, timeout=30)
        for path in rootfs.rglob("*.desktop"):
            run(["desktop-file-validate", str(path)])
        reports = audit_binary_payload(rootfs)
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
            package, native = package_arch(archive, rootfs, reports, dependencies, work, bundle, version)
        else:
            package, native = package_rpm(archive, rootfs, dependencies, work, bundle, version)
        # Keep a verifiable staged rootfs for integration audits, in addition to the native payload.
        shutil.copyfile(archive, bundle / "rootfs.tar.gz")
        app_sources(artifacts, epoch)
        sources = [{"file": "sources/" + path.name, "license": "MIT AND GPL-3.0-or-later AND BSD-3-Clause" if path.name.startswith("peripheral-hub-") else pipewire_client["license"] if path.name.startswith("pipewire-1.0.5-") else "BSD-3-Clause" if "evdev" in path.name else "GPL-3.0-or-later"} for path in sorted(artifacts.iterdir())]
        manifest = {
            "schema": 1, "app": {"name": APP, "version": version}, "architecture": "x86_64", "init": "systemd",
            "target": {"id": target["id"], "os_release": release, "image": target["image"], "image_digest": image_digest, "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "source_date_epoch": epoch},
            "package": {"file": package.name, "format": target["format"], "version": version + "-1", "signed": False},
            "requirements": {"elf": reports, "native": native, "resolved_packages": resolved, "python": ".".join(map(str, sys.version_info[:3]))},
            "toolchain": {"rustc": run(["rustc", "--version"]).stdout.strip(), "cargo": run(["cargo", "--version"]).stdout.strip(), "node": run(["node", "--version"]).stdout.strip()},
            "source_revision": run(["git", "rev-parse", "HEAD"], cwd=REPO).stdout.strip(),
            "source_tree_dirty": bool(run(["git", "status", "--porcelain"], cwd=REPO).stdout.strip()),
            "components": {"gui": "usr/bin/peripheral-hub", "agent": {"binary": "usr/bin/peripheral-hub-remapping-agent", "cargo_features": "--no-default-features"}, "audio_worker": {"binary": "usr/bin/peripheral-hub-audio-worker", "cargo_features": "--no-default-features"}, "pipewire_client": pipewire_client, "input_remapper": {"version": "2.2.1", "commit": "e9a87d13480c3b1dee654b296578a8f4e2cd31d6", "bus": "io.peripheralhub.InputRemapper"}, "evdev": private_evdev, "headsetcontrol": headset},
            "sources": sources, "validation": {"software": software, "elf": "passed", "python_imports": "passed", "native_smoke": "not-run", "linux_hardware": "not-run", "graphical_login_reboot": "not-run"},
        }
        write_manifest(bundle, manifest)
    print(f"Built native bundle: {bundle}; installation remains gated until smoke tests pass")
    return bundle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=[item["id"] for item in load_targets()["targets"]])
    parser.add_argument("--output", type=Path, default=HERE / "out/quarantine")
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
