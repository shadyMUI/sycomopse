#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Explicitly authenticated system phase. Does not run any user service command."""

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

if sys.platform == "linux":
    import pwd

# -I ignores PYTHONPATH, user site packages and the current working directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import (
    APP, ENGINE, STATE, PackagingError, bundle_file, digest, json_file, load_targets,
    package_name, parse_os_release, pgp_fingerprint, regular_bytes, resolve_packages,
    run, target_for, verify_bundle, version_at_least, write_json,
)
from install import facts_for, installed, system_plan


def owned_file(path, data, record, mode=0o644):
    path = Path(path)
    if path.is_symlink():
        raise PackagingError(f"Refusing to replace a symlink: {path}")
    if path.exists():
        if regular_bytes(path, len(data) + 1) != data:
            raise PackagingError(f"Existing configuration differs; not overwriting {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    for parent in path.parents:
        if parent.is_symlink():
            raise PackagingError(f"Symlink in administrative path: {parent}")
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), mode), "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    record.setdefault("created_files", {})[str(path)] = digest(data)


def repository_setup(target, spec, key_data, record):
    expected = load_targets()["repository_keys"][spec["key"]]
    if pgp_fingerprint(key_data) != expected:
        raise PackagingError("Repository public key fingerprint mismatch")
    if spec["kind"] in ("ppa", "obs-deb"):
        key_path = Path("/etc/apt/keyrings/peripheral-hub-openrazer.asc")
        owned_file(key_path, key_data, record)
        suite = f"{target['codename']} main" if spec["kind"] == "ppa" else "/"
        line = f"deb [arch=amd64 signed-by={key_path}] {spec['url']}/ {suite}\n"
        owned_file("/etc/apt/sources.list.d/peripheral-hub-openrazer.list", line.encode(), record)
        run(["apt-get", "update"], capture=False, timeout=600)
    else:
        key_path = Path("/etc/peripheral-hub/repository-keys/openrazer.asc")
        owned_file(key_path, key_data, record)
        text = f"[peripheral-hub-openrazer]\nname=Peripheral Hub - official OpenRazer hardware:razer\nbaseurl={spec['url']}/\nenabled=1\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=file://{key_path}\n"
        directory = "/etc/yum.repos.d" if target["manager"] == "dnf" else "/etc/zypp/repos.d"
        owned_file(Path(directory) / "peripheral-hub-openrazer.repo", text.encode(), record)
        # Import an existing upstream public key, never create a signing/MOK key.
        run(["rpm", "--import", str(key_path)], capture=False)
        if target["manager"] == "dnf":
            run(["dnf", "makecache", "--refresh"], capture=False, timeout=600)
        else:
            run(["zypper", "refresh"], capture=False, timeout=600)


def check_repository_policy(target, etc=Path("/etc")):
    if target["manager"] == "apt":
        policy = run(["apt-config", "dump"]).stdout
        if re.search(r'(AllowUnauthenticated|AllowInsecureRepositories|AllowDowngradeToInsecureRepositories)\s+"(?:true|1)"', policy, re.I):
            raise PackagingError("APT insecure/unauthenticated policy is enabled; refusing an unverified transaction")
    if target["manager"] == "pacman":
        if "Never" in run(["pacman-conf", "SigLevel"]).stdout:
            raise PackagingError("Pacman repository signature checks are disabled")
    if target["manager"] in ("dnf", "zypper"):
        import configparser
        default = True
        main = etc / "dnf/dnf.conf"
        if target["manager"] == "dnf" and main.is_file():
            config = configparser.ConfigParser(interpolation=None)
            config.read_string(regular_bytes(main, 128 * 1024).decode())
            default = config.getboolean("main", "gpgcheck", fallback=True)
        directory = etc / ("yum.repos.d" if target["manager"] == "dnf" else "zypp/repos.d")
        for path in directory.glob("*.repo"):
            config = configparser.ConfigParser(interpolation=None)
            config.read_string(regular_bytes(path, 128 * 1024).decode())
            for section in config.sections():
                if config.getboolean(section, "enabled", fallback=True) and not config.getboolean(section, "gpgcheck", fallback=default):
                    raise PackagingError(f"Enabled repository {section} has signature checks disabled; not altering it")


def native_install(target, package, packages, choices, repair=False, yes=False):
    manager = target["manager"]
    if manager == "apt":
        argv = ["apt-get", "install", "--no-install-recommends", *(["--reinstall"] if repair else []), str(package), *packages]
        simulation = run(["apt-get", "--simulate", *argv[1:]], timeout=600).stdout
        removed = re.findall(r"^Remv\s+(\S+)", simulation, re.M)
        allowed = {"pulseaudio", "pulseaudio-module-bluetooth", "pulseaudio-utils"} if choices.get("audio_switch") else set()
        if choices.get("session_manager"):
            allowed.add("pipewire-media-session")
        if set(removed) - allowed:
            raise PackagingError("APT would remove unapproved packages: " + ", ".join(set(removed) - allowed))
        if re.search(r"^Inst\s+linux-image-", simulation, re.M):
            raise PackagingError("Transaction would install/change a kernel; this installer never does that")
        print(simulation)
        run([*argv, *(["--yes"] if yes else [])], capture=False, timeout=3600)
    elif manager == "pacman":
        # Never -Sy followed by -S (partial upgrade), and never silently -Syu the OS/kernel.
        # A stale exact kernel/header repository pair is rejected before this transaction.
        run(["pacman", "-S", "--needed", *(["--noconfirm"] if yes else []), "--", *packages], capture=False, timeout=3600)
        run(["pacman", "-U", *(["--noconfirm"] if yes else []), "--", str(package)], capture=False, timeout=600)
    elif manager == "dnf":
        if repair:
            run(["dnf", "reinstall", *(["--assumeyes"] if yes else []), str(package)], capture=False, timeout=3600)
        run(["dnf", "install", *(["--assumeyes"] if yes else []), str(package), *packages], capture=False, timeout=3600)
    else:
        run(["zypper", *(["--non-interactive"] if yes else []), "install", *(["--force"] if repair else []), str(package), *packages], capture=False, timeout=3600)


def system_unit(unit, record, restart=False):
    load = run(["systemctl", "show", unit, "--property=LoadState", "--value"]).stdout.strip()
    if load != "loaded":
        raise PackagingError(f"System unit not installed: {unit}")
    state = run(["systemctl", "is-enabled", unit], check=False).stdout.strip()
    if state.startswith("masked"):
        raise PackagingError(f"{unit} is masked externally; not overriding it")
    operation = ["start", unit] if state in ("static", "indirect", "generated", "transient") else ["enable", "--now", unit]
    run(["systemctl", *operation], capture=False)
    if restart:
        run(["systemctl", "restart", unit], capture=False)
    record.setdefault("services", {})[unit] = "running" if run(["systemctl", "is-active", unit], check=False).returncode == 0 else "failed"
    if record["services"][unit] == "failed":
        raise PackagingError(f"{unit} did not become active")


def configure_razer(target, facts, username, record):
    kernel = facts["kernel"]
    header = Path("/lib/modules") / kernel / "build/include/config/kernel.release"
    if not header.is_file():
        header = Path("/usr/lib/modules") / kernel / "build/include/config/kernel.release"
    if not header.is_file() or regular_bytes(header, 256).decode().strip() != kernel:
        record["pending"].append("openrazer: matching running-kernel headers unavailable; no other kernel was installed")
        return
    if target["manager"] == "zypper":
        system_unit("dkms.service", record)
    response = run(["dkms", "autoinstall", "-k", kernel], check=False, capture=False, timeout=1200)
    status = run(["dkms", "status", "-k", kernel], check=False).stdout
    if response.returncode or not re.search(r"openrazer[^\n]*installed", status, re.I):
        record["pending"].append("openrazer: DKMS failed for the running kernel; inspect native package/DKMS logs before reboot")
    import grp
    try:
        group = grp.getgrnam("plugdev")
    except KeyError:
        run(["groupadd", "--system", "plugdev"], capture=False)
        record["plugdev_group_created"] = True
        group = grp.getgrnam("plugdev")
    user = pwd.getpwnam(username)
    if username not in group.gr_mem and user.pw_gid != group.gr_gid:
        run(["usermod", "--append", "--groups", "plugdev", username], capture=False)
        record["pending"].append("openrazer: pending logout/login for the new plugdev supplementary group")
        record["plugdev_user_added"] = username
    if facts.get("secure_boot"):
        record["pending"].append("openrazer: Secure Boot signing/MOK enrollment pending; no keys created and no firmware policy changed")
    modules = ["razerkbd", "razermouse", "razeraccessory"]
    loaded = regular_bytes("/proc/modules", 1024 * 1024).decode()
    if not any(re.search(rf"^{module}\s", loaded, re.M) for module in modules):
        record["pending"].append("openrazer: pending reboot to load the DKMS driver; installer does not silently reload HID devices")


def remove_owned_records(record):
    for name, expected in list(record.get("created_files", {}).items()):
        allowed = name.startswith(("/etc/apt/keyrings/peripheral-hub-", "/etc/apt/sources.list.d/peripheral-hub-", "/etc/yum.repos.d/peripheral-hub-", "/etc/zypp/repos.d/peripheral-hub-", "/etc/peripheral-hub/repository-keys/"))
        path = Path(name)
        if not allowed:
            raise PackagingError("Invalid installer-owned file record")
        if path.exists() and not path.is_symlink() and digest(regular_bytes(path, 128 * 1024)) == expected:
            path.unlink()
        elif path.exists():
            record["pending"].append(f"Preserved externally modified repository configuration: {name}")
    # Imported public keys/groups may be used by other software and are never removed.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--key", type=Path)
    args = parser.parse_args()
    record = None
    try:
        if sys.platform != "linux" or os.geteuid() != 0:
            raise PackagingError("System phase requires explicitly authorized root on Linux")
        request = json.loads(regular_bytes(args.request, 65536))
        uid = request.get("uid")
        authorization_uid = os.environ.get("SUDO_UID", os.environ.get("PKEXEC_UID"))
        if type(uid) is not int or uid <= 0 or authorization_uid is None or int(authorization_uid) != uid:
            raise PackagingError("Caller UID does not match the sudo/Polkit authorization")
        regular_bytes(args.request, 65536, owner=uid)
        user = pwd.getpwuid(uid)
        if request.get("schema") != 1 or request.get("action") not in ("install", "repair", "uninstall"):
            raise PackagingError("Invalid administrative request")
        if type(request.get("yes")) is not bool or type(request.get("purge")) is not bool:
            raise PackagingError("Invalid native transaction confirmation/purge flags")
        choices = request.get("choices")
        allowed = {"audio", "openrazer", "audio_switch", "session_manager", "ratbag", "repository", "unsigned_dkms"}
        if not isinstance(choices, dict) or set(choices) != allowed or not all(type(value) is bool for value in choices.values()):
            raise PackagingError("Only explicit, known backend consent flags are accepted")
        manifest, manifest_hash = verify_bundle(args.bundle, request.get("manifest_sha256"))
        release = parse_os_release(regular_bytes("/etc/os-release", 65536).decode())
        target = target_for(release)
        if target["id"] != request["target"] or target["id"] != manifest["target"]["id"]:
            raise PackagingError("Administrative target differs from verified user preflight")
        facts = facts_for(target, user=False)
        if facts["init"] != "systemd" or facts["arch"] != "x86_64" or facts["immutable"]:
            raise PackagingError("Unsupported administrative host; no transaction performed")
        check_repository_policy(target)
        if STATE.exists():
            record = json_file(STATE)
            if record.get("schema") != 1 or record.get("app") != APP:
                raise PackagingError("Unknown installer state schema; repair refused without overwriting it")
        else:
            record = {"schema": 1, "app": APP, "created_files": {}, "services": {}}
        record.update(status="installing", uid=uid, target=target["id"], version=manifest["app"]["version"], manifest_sha256=manifest_hash, choices=choices, pending=[])
        STATE.parent.mkdir(parents=True, exist_ok=True)
        if STATE.parent.is_symlink():
            raise PackagingError("Installer state directory is a symlink")
        write_json(STATE, record)
        with tempfile.TemporaryDirectory(prefix="peripheral-hub-verified-", dir="/var/tmp") as temporary:
            snapshot = Path(temporary)
            # Copy verified bytes to a root-owned directory and recheck before native managers open them.
            source = bundle_file(args.bundle, manifest["package"]["file"])
            data = regular_bytes(source, manifest["files"][manifest["package"]["file"]]["size"] + 1)
            if digest(data) != manifest["files"][manifest["package"]["file"]]["sha256"]:
                raise PackagingError("Package changed after preflight")
            package = snapshot / source.name
            package.write_bytes(data)
            package.chmod(0o644)
            snapshot.chmod(0o755)
            plan = system_plan(target, facts, choices, request["action"], str(package))
            record["pending"] += plan["pending"]
            if request["action"] == "uninstall":
                for operation in plan["commands"]:
                    if operation["argv"][0] == "systemctl":
                        run(operation["argv"], check=False, capture=False)
                    elif installed(target["manager"], APP):
                        run(operation["argv"], capture=False, timeout=1200)
                if request.get("purge"):
                    remove_owned_records(record)
                record.update(status="removed", package_installed=False)
            else:
                if target["manager"] == "apt":
                    run(["apt-get", "update"], capture=False, timeout=600)
                elif target["manager"] == "dnf":
                    run(["dnf", "makecache", "--refresh"], capture=False, timeout=600)
                elif target["manager"] == "zypper":
                    run(["zypper", "refresh"], capture=False, timeout=600)
                # Query real signed native metadata, including per-interpreter openSUSE names.
                resolved, missing = resolve_packages(target, plan["packages"])
                razer = next((entry["version"] for entry in resolved if entry["name"] in ("openrazer-daemon", "openrazer-meta")), facts.get("openrazer_version", ""))
                if plan["repository"] and (missing or not version_at_least(razer, "3.12.4")):
                    if not args.key or digest(regular_bytes(args.key, 128 * 1024, owner=uid)) != request.get("repository_key_sha256"):
                        raise PackagingError("Verified upstream repository key missing/changed")
                    repository_setup(target, plan["repository"], regular_bytes(args.key, 128 * 1024, owner=uid), record)
                    resolved, missing = resolve_packages(target, plan["packages"])
                if missing:
                    raise PackagingError("Packages unavailable in signed target repositories: " + ", ".join(missing))
                razer = next((entry["version"] for entry in resolved if entry["name"] in ("openrazer-daemon", "openrazer-meta")), "")
                if razer and not any(version_at_least(version, "3.12.4") for version in (razer, facts.get("openrazer_version", ""))):
                    raise PackagingError("Signed repositories do not provide the required OpenRazer >= 3.12.4; no old driver silently substituted")
                if target["manager"] == "pacman" and not facts["kernel_headers_match"] and choices["openrazer"]:
                    headers = facts["kernel_pkgbase"] + "-headers"
                    version = next((item["version"] for item in resolved if item["name"] == headers), None)
                    if version != facts.get("kernel_package_version"):
                        raise PackagingError("Arch/CachyOS repository headers do not match the actual running kernel package; no kernel/headers substituted")
                record["resolved_packages"] = resolved
                write_json(STATE, record)
                previous = run(["systemctl", "is-active", ENGINE], check=False).returncode == 0
                targets = []
                for item in resolved:
                    constraint = item["requirement"][len(package_name(item["requirement"])):]
                    targets.append(item["name"] + constraint if constraint.startswith("=") else item["name"])
                native_install(target, package, targets, choices, repair=request["action"] == "repair", yes=request.get("yes", False))
                record["package_installed"] = bool(installed(target["manager"], APP))
                run(["systemctl", "daemon-reload"], capture=False)
                run(["udevadm", "control", "--reload-rules"], capture=False)
                # Do not trigger/rebind all existing HID devices; uaccess changes can require replug.
                run(["/usr/lib/peripheral-hub/remapper-service", "--check-imports"], capture=False, timeout=30)
                system_unit(ENGINE, record, restart=previous)
                if choices["ratbag"]:
                    system_unit("ratbagd.service", record)
                    system_unit("upower.service", record)
                if choices["openrazer"] and not any(item.startswith("openrazer:") for item in plan["pending"]):
                    configure_razer(target, facts, user.pw_name, record)
                if Path("/usr/lib/peripheral-hub/headsetcontrol").is_file():
                    record["pending"].append("headsetcontrol: reconnect existing headsets if new uaccess rules have not taken effect")
                record["status"] = "pending" if record["pending"] else "running"
        write_json(STATE, record)
        print(json.dumps(record, indent=2, sort_keys=True))
        return 0
    except (PackagingError, OSError, ValueError) as error:
        if record is not None:
            record.update(status="failed", error=str(error))
            record["package_installed"] = bool(installed(target["manager"], APP)) if "target" in locals() else False
            write_json(STATE, record)
        print(f"System phase failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
