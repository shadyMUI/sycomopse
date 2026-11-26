#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Online native installer. Always enter as the normal desktop user."""

import argparse
import json
import os
import platform
import shutil
import sys
import tempfile
from pathlib import Path

from common import (
    AGENT, APP, ENGINE, HEADSET, HEADSET_REPLUG_NOTICE, HERE, NATIVE_FAILURE, NATIVE_STATE, STATE,
    PackagingError, build_target, digest, https_bytes,
    json_file, kernel_packages, load_targets, native_failure, package_name, parse_os_release,
    pgp_fingerprint, probe_headset, regular_bytes, resolve_packages, run,
    repository_version, target_for, verify_bundle, version_at_least, write_json,
)

# The first elevated code uses only the stdlib. No user-writable helper is
# imported/executed until all its bytes match the manifest approved by preflight.
ROOT_BOOTSTRAP = r'''
import hashlib, json, os, stat, subprocess, sys, tempfile
from pathlib import Path, PurePosixPath
if sys.platform != "linux" or os.geteuid() != 0:
    raise SystemExit("Administrative staging requires explicitly authenticated Linux root")
bundle, request, expected = sys.argv[1:4]
key = sys.argv[4] if len(sys.argv) == 5 else None
def read(path, limit):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > limit:
            raise SystemExit("Untrusted administrative staging input")
        data = stream.read(limit + 1)
        if len(data) > limit:
            raise SystemExit("Administrative staging input exceeds limit")
        return data
raw = read(Path(bundle) / "manifest.json", 8 * 1024 * 1024)
if hashlib.sha256(raw).hexdigest() != expected:
    raise SystemExit("Manifest changed before administrative authorization")
manifest = json.loads(raw)
files = manifest.get("files")
if not isinstance(files, dict) or not files or len(files) > 10000:
    raise SystemExit("Invalid administrative file inventory")
with tempfile.TemporaryDirectory(prefix="peripheral-hub-authorized-", dir="/var/tmp") as temporary:
    destination = Path(temporary)
    for name, entry in files.items():
        if not isinstance(name, str) or "\\" in name or ":" in name or any(ord(char) < 32 for char in name):
            raise SystemExit("Invalid administrative artifact name")
        relative = PurePosixPath(name)
        if relative.is_absolute() or any(part in ("", ".", "..") for part in name.split("/")):
            raise SystemExit("Administrative artifact escapes bundle")
        size = entry.get("size")
        if type(size) is not int or not 0 <= size <= 2 ** 31:
            raise SystemExit("Invalid administrative artifact size")
        data = read(Path(bundle).joinpath(*relative.parts), size + 1)
        if len(data) != size or hashlib.sha256(data).hexdigest() != entry.get("sha256"):
            raise SystemExit("Administrative artifact checksum changed")
        target = destination.joinpath(*relative.parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(0o644)
    (destination / "manifest.json").write_bytes(raw)
    (destination / "manifest.sha256").write_text(expected + "  manifest.json\n", encoding="ascii")
    command = ["/usr/bin/python3", "-I", str(destination / "system.py"), "--bundle", str(destination), "--request", request]
    if key:
        command += ["--key", key]
    raise SystemExit(subprocess.run(command, check=False).returncode)
'''


def installed(manager, name, details=False):
    commands = {
        "apt": ["dpkg-query", "-W", "-f=${db:Status-Status} ${Version}", name],
        "pacman": ["pacman", "-Q", name],
        "dnf": ["rpm", "-q", "--qf", "%{VERSION}-%{RELEASE}", name],
        "zypper": ["rpm", "-q", "--qf", "%{VERSION}-%{RELEASE}", name],
    }
    result = run(commands[manager], check=False)
    if result.returncode:
        missing = {
            "apt": f"dpkg-query: no packages found matching {name}",
            "pacman": f"error: package '{name}' was not found",
            "dnf": f"package {name} is not installed", "zypper": f"package {name} is not installed",
        }[manager]
        if result.returncode == 1 and (result.stdout + "\n" + result.stderr).strip() == missing:
            return None
        raise PackagingError(f"Cannot determine installation of {name}: package status unknown ({result.stderr.strip() or result.stdout.strip()})")
    fields = result.stdout.split()
    if not fields:
        raise PackagingError(f"Cannot determine installation of {name}: empty package query")
    state = fields[0] if manager == "apt" else "installed"
    if state in ("not-installed", "config-files"):
        return None
    if manager == "apt" and (len(fields) != 2 or state not in ("installed", "unpacked", "half-installed", "half-configured", "triggers-awaited", "triggers-pending")):
        raise PackagingError(f"Cannot determine installation of {name}: unknown dpkg status")
    version = fields[-1]
    if details:
        return {"state": state, "version": version}
    return version if state == "installed" else None


def facts_for(target, user=True):
    release = run(["uname", "-r"]).stdout.strip()
    modules = Path("/usr/lib/modules") / release
    if not modules.is_dir():
        modules = Path("/lib/modules") / release
    pkgbase = modules / "pkgbase"
    kernel_header = modules / "build/include/config/kernel.release"
    facts = {
        "arch": platform.machine(), "init": regular_bytes("/proc/1/comm", 128).decode().strip(),
        "uid": os.geteuid(), "kernel": release,
        "kernel_pkgbase": regular_bytes(pkgbase, 256).decode().strip() if pkgbase.is_file() else "",
        "kernel_headers_match": kernel_header.is_file() and regular_bytes(kernel_header, 256).decode().strip() == release,
        "immutable": Path("/run/ostree-booted").exists(),
        "secure_boot": False, "session": bool(os.environ.get("XDG_RUNTIME_DIR") and os.environ.get("DBUS_SESSION_BUS_ADDRESS")),
        "python": ".".join(map(str, sys.version_info[:3])),
        "pulse_audio": bool(installed(target["manager"], "pulseaudio")),
        "media_session": bool(installed(target["manager"], "pipewire-media-session")),
        "groups": [],
    }
    import grp
    facts["groups"] = [grp.getgrgid(gid).gr_name for gid in os.getgroups()] if user else []
    if shutil.which("mokutil"):
        response = run(["mokutil", "--sb-state"], check=False)
        facts["secure_boot"] = "SecureBoot enabled" in response.stdout
    else:
        for variable in Path("/sys/firmware/efi/efivars").glob("SecureBoot-*"):
            try:
                value = variable.read_bytes()
                facts["secure_boot"] = len(value) >= 5 and value[4] == 1
            except OSError:
                raise PackagingError("Cannot determine Secure Boot state; run preflight with readable firmware metadata")
    if target["manager"] == "zypper":
        response = run(["rpm", "-q", "--whatprovides", f"kernel-uname-r = {release}", "--qf", "%{NAME}\n%{VERSION}-%{RELEASE}\n"], check=False)
        parts = response.stdout.strip().splitlines()
        if response.returncode == 0 and len(parts) == 2 and parts[0].startswith("kernel-"):
            facts.update(kernel_flavor=parts[0][7:], kernel_package_version=parts[1])
    razer_version = installed(target["manager"], "openrazer-daemon")
    if target["manager"] == "pacman" and facts["kernel_pkgbase"]:
        facts["kernel_package_version"] = installed("pacman", facts["kernel_pkgbase"])
    facts["openrazer_version"] = razer_version or ""
    razer_package = "openrazer-meta" if target["manager"] in ("dnf", "zypper") else "openrazer-daemon"
    facts["openrazer_repository_version"] = repository_version(target["manager"], razer_package) or ""
    system_cli = shutil.which("headsetcontrol") if user else None
    facts["headsetcontrol"] = probe_headset(system_cli) if system_cli else {"configured": False, "reason": "System CLI is not installed"}
    return facts


def preflight(target, facts, manifest=None):
    if facts.get("uid") == 0:
        raise PackagingError("Start install.py as your normal user, not sudo/root. Authentication is requested only for the system phase")
    if sys.version_info < (3, 10) or not version_at_least(facts.get("python", ".".join(map(str, sys.version_info[:3]))), "3.10"):
        raise PackagingError("The installer and root engine require Python >= 3.10")
    if facts.get("arch") != "x86_64" or facts.get("init") != "systemd" or facts.get("immutable"):
        raise PackagingError("Only mutable x86_64 systems booted with systemd are supported")
    if manifest:
        built = manifest.get("target", {})
        if built.get("id") != target["id"] or manifest["package"]["format"] != target["format"]:
            raise PackagingError("This bundle was built for a different distribution/version")
        if build_target(built, manifest["package"]["format"])["id"] != target["id"]:
            raise PackagingError("This bundle was built for a different distribution/version")
        if manifest.get("validation", {}).get("native_smoke") != "passed":
            raise PackagingError("Bundle has not passed native install/upgrade/repair smoke tests")
    if not facts.get("kernel"):
        raise PackagingError("Running kernel release is required")


def needs_razer_repository(target, facts):
    return bool(target.get("repository")) and not any(version_at_least(facts.get(key, ""), "3.12.4") for key in ("openrazer_version", "openrazer_repository_version"))


def system_plan(target, facts, choices, action="install", package="PACKAGE"):
    effects = []
    pending = []
    packages = [package_name(item) for item in target["runtime"]]
    packages += target["headset"]
    if choices.get("audio", True):
        if facts.get("media_session") and not choices.get("session_manager"):
            pending.append("audio: pipewire-media-session replacement was not authorized")
        else:
            packages += target["audio"]
            if facts.get("media_session"):
                effects.append("Replace pipewire-media-session with WirePlumber; preserve existing configuration")
            if facts.get("pulse_audio"):
                if choices.get("audio_switch"):
                    packages += target["pulse"]
                    effects.append("Replace the PulseAudio server with PipeWire's PulseAudio protocol server; no audio configuration is overwritten")
                else:
                    pending.append("audio: PulseAudio replacement was not authorized")
            else:
                packages += target["pulse"]
    else:
        pending.append("audio: configuration declined")
    if choices.get("ratbag"):
        packages += target["ratbag"]
        effects.append("Start ratbagd (USB/HID probe) and UPower; never replace existing ratbag configuration")
    else:
        pending.append("ratbag: device probing was not authorized")
    repository = None
    if choices.get("openrazer", True):
        if facts.get("secure_boot") and not choices.get("unsigned_dkms"):
            pending.append("openrazer: Secure Boot is enabled and unsigned DKMS installation was not authorized")
        else:
            packages += target["razer"]
            try:
                packages += kernel_packages(target, facts)
            except PackagingError as error:
                pending.append(f"openrazer: {error}")
                packages = [name for name in packages if name not in target["razer"]]
            if not any(item.startswith("openrazer:") for item in pending):
                effects.append(f"Install native OpenRazer/DKMS hooks and verify kernel {facts['kernel']} with exact headers; append this user to plugdev. Native hooks can also process other already-installed kernels")
                if needs_razer_repository(target, facts):
                    if choices.get("repository"):
                        repository = target["repository"]
                        effects.append("Add a scoped, signed official OpenRazer repository only if distro packages are too old/unavailable")
                    else:
                        pending.append("openrazer: current signed upstream repository was not authorized")
                        packages = [name for name in packages if name not in target["razer"] and not name.startswith(("linux-headers-", "kernel-devel", "kernel-"))]
                if facts.get("secure_boot"):
                    pending.append("openrazer: pending Secure Boot module-signing/MOK enrollment via the distro; no keys are created and Secure Boot is never disabled")
    else:
        pending.append("openrazer: configuration declined")
    commands = []
    if action == "uninstall":
        commands.append({"scope": "root", "argv": ["systemctl", "disable", "--now", ENGINE]})
        native = {
            "apt": ["apt-get", "remove", APP], "pacman": ["pacman", "-R", "--", APP],
            "dnf": ["dnf", "remove", "--noautoremove", APP], "zypper": ["zypper", "remove", APP],
        }
        commands.append({"scope": "root", "argv": native[target["manager"]]})
        effects = ["Remove only the app package and its private engine; preserve user data and all external packages/services"]
        repository = None
        pending = []
        packages = []
    else:
        packages = list(dict.fromkeys(packages))
        native = {
            "apt": ["apt-get", "install", "--no-install-recommends", package, *packages],
            "pacman": ["pacman", "-S", "--needed", "--", *packages],
            "dnf": ["dnf", "install", package, *packages],
            "zypper": ["zypper", "install", package, *packages],
        }
        if target["manager"] != "pacman":
            commands.append({"scope": "root", "argv": native[target["manager"]]})
        else:
            commands += [{"scope": "root", "argv": native["pacman"]}, {"scope": "root", "argv": ["pacman", "-U", "--", package]}]
        commands += [{"scope": "root", "argv": ["systemctl", "daemon-reload"]}, {"scope": "root", "argv": ["systemctl", "enable", "--now", ENGINE]}]
        if choices.get("ratbag"):
            commands += [{"scope": "root", "argv": ["systemctl", "start", unit]} for unit in ("ratbagd.service", "upower.service")]
        if target["manager"] == "zypper" and choices.get("openrazer", True) and not any(item.startswith("openrazer:") for item in pending):
            commands.append({"scope": "root", "argv": ["systemctl", "start", "dkms.service"]})
        effects.append("Started distro units are also enabled only when their UnitFileState supports it; existing masks are preserved")
    return {"target": target["id"], "action": action, "packages": packages, "repository": repository, "effects": effects, "pending": pending, "commands": commands}


def user_commands(plan, facts, choices):
    if plan["action"] == "uninstall":
        return [
            {"scope": "user", "argv": ["/usr/bin/peripheral-hub-remapping-agent", "--disable-all"]},
            {"scope": "user", "argv": ["systemctl", "--user", "disable", "--now", AGENT]},
        ]
    commands = [{"scope": "user", "argv": ["systemctl", "--user", "daemon-reload"]}, {"scope": "user", "argv": ["systemctl", "--user", "enable", "--now", AGENT]}]
    if choices.get("openrazer", True) and not any(item.startswith("openrazer:") for item in plan["pending"]):
        commands.append({"scope": "user", "unit": "openrazer-daemon.service"})
    if choices.get("audio", True) and not any(item.startswith("audio:") for item in plan["pending"]):
        commands += [{"scope": "user", "unit": unit} for unit in ("pipewire.socket", "pipewire-pulse.socket", "wireplumber.service")]
    return commands


def activate_user_unit(unit):
    load = run(["systemctl", "--user", "show", unit, "--property=LoadState", "--value"], check=False)
    if load.returncode or load.stdout.strip() != "loaded":
        raise PackagingError(f"User unit missing: {unit}")
    state = run(["systemctl", "--user", "is-enabled", unit], check=False).stdout.strip()
    if state in ("masked", "masked-runtime"):
        raise PackagingError(f"User unit {unit} is masked by the administrator; not overriding it")
    # Static units and DBus-activated services are startable, not enableable.
    operation = ["start", unit] if state in ("static", "indirect", "generated", "transient") else ["enable", "--now", unit]
    run(["systemctl", "--user", *operation], capture=False)


def ask(text, approved, args):
    if approved:
        return True
    if args.yes or not sys.stdin.isatty():
        return False
    return input(f"{text} [y/N] ").strip().lower() in ("y", "yes")


def choices_for(args, target, facts):
    choices = {
        "audio": not args.without_audio, "openrazer": not args.without_openrazer,
        "audio_switch": args.consent_audio_switch, "session_manager": args.consent_session_manager,
        "ratbag": args.consent_ratbag, "repository": args.consent_repository,
        "unsigned_dkms": args.consent_unsigned_dkms,
    }
    if not args.dry_run and args.action in ("install", "repair"):
        choices["ratbag"] = ask("Authorize ratbagd to probe USB/HID mice?", choices["ratbag"], args)
        if facts.get("pulse_audio") and choices["audio"]:
            choices["audio_switch"] = ask("Authorize replacing the PulseAudio server with PipeWire-Pulse?", choices["audio_switch"], args)
        if facts.get("media_session") and choices["audio"]:
            choices["session_manager"] = ask("Authorize replacing pipewire-media-session with WirePlumber?", choices["session_manager"], args)
        if needs_razer_repository(target, facts) and choices["openrazer"]:
            choices["repository"] = ask("Authorize the signed official OpenRazer repository, only if required?", choices["repository"], args)
        if facts.get("secure_boot") and choices["openrazer"]:
            choices["unsigned_dkms"] = ask("Authorize DKMS installation while Secure Boot is enabled, with module signing/enrollment left pending?", choices["unsigned_dkms"], args)
    return choices


def print_plan(plan, manifest, facts):
    print(f"Sycomopse 0.1.0 | {plan['target']} | {plan['action']} | kernel {facts['kernel']}")
    if plan.get("compatible_host"):
        host = plan["host_os_release"]
        print(f"Host: {host.get('PRETTY_NAME', host['ID'] + ' ' + host.get('VERSION_ID', ''))}; explicit Noble-base compatibility, not a native Pop build")
    print("Native packages: " + ", ".join(plan["packages"]))
    if plan["repository"]:
        print("Official repository: " + plan["repository"]["url"])
    for source in load_targets()["sources"].values():
        print("Source: " + source)
    for effect in plan["effects"]:
        print("Effect: " + effect)
    for command in plan["commands"]:
        print("SYSTEM: " + " ".join(command["argv"]))
    for command in plan["user_commands"]:
        if "argv" in command:
            print("USER: " + " ".join(command["argv"]))
        else:
            print("USER UNIT: " + command["unit"] + "; start and enable only if supported, preserving masks")
    for message in plan["pending"]:
        print("NOT READY: " + message)
    headset = facts.get("headsetcontrol", {})
    if not headset.get("configured"):
        candidate = (manifest or {}).get("components", {}).get("headsetcontrol", {})
        print("HeadsetControl: " + candidate.get("reason", headset.get("reason", "not configured")))
    print("Repository signatures remain enabled. Local artifacts are checksum-verified, not automatically signed; stricter native signing policies are respected.")


def installation_status(target, facts):
    state = json_file(STATE) if STATE.exists() else None
    checkpoint = json_file(NATIVE_STATE) if NATIVE_STATE.exists() else None
    for record, identity in ((state, {"app": APP}), (checkpoint, {"engine": ENGINE, "format": target["format"]})):
        if record is None:
            continue
        if record.get("schema") != 1 or any(record.get(key) != value for key, value in identity.items()):
            raise PackagingError("Unknown installation checkpoint; status cannot be verified")
        for field in ("pending", "notices"):
            messages = record.get(field, [])
            if not isinstance(messages, list) or not all(isinstance(item, str) for item in messages):
                raise PackagingError("Malformed installation checkpoint; status cannot be verified")
    query_error = None
    failure_error = None
    try:
        failure = native_failure(NATIVE_FAILURE)
    except PackagingError as error:
        failure, failure_error = None, str(error)
    try:
        package = installed(target["manager"], APP, details=True)
    except PackagingError as error:
        package, query_error = None, str(error)
    agent_running = run(["systemctl", "--user", "is-active", AGENT], check=False).returncode == 0
    engine_running = run(["systemctl", "is-active", ENGINE], check=False).returncode == 0
    pending, notices = [], list((state or {}).get("notices", []))
    headset = None
    if package:
        if failure_error:
            pending.append("native-package: configuration outcome is unknown: " + failure_error)
        elif failure:
            pending.append("native-package: configure failed: " + failure["error"] if failure["status"] == "failed" else "native-package: configure has not completed")
        if package["state"] != "installed":
            pending.append("native-package: package configuration is incomplete (" + package["state"] + ")")
        if state and state.get("status") not in ("removed", "not-installed"):
            for message in state.get("pending", []):
                # Older schema-1 records stored this conditional advice as a
                # blocker. A fresh user probe, not that warning, decides readiness.
                (notices if message == HEADSET_REPLUG_NOTICE else pending).append(message)
            if state.get("status") == "failed":
                pending.append("installer: previous system transaction failed: " + str(state.get("error", "inspect the system record")))
        if checkpoint and (not engine_running or checkpoint.get("status") in ("failed", "removing", "remove-failed")):
            pending += checkpoint.get("pending", [])
            if checkpoint.get("status") in ("removing", "remove-failed"):
                pending.append("native-package: removal checkpoint is incomplete")
            if checkpoint.get("status") == "failed":
                pending.append("native-package: failed native checkpoint: " + str(checkpoint.get("error", "inspect native record")))
        if not agent_running:
            pending.append("user-services: remapping agent is not active in this user's session")
        if not engine_running:
            pending.append("private-engine: system backend is not active")
        headset = facts.get("headsetcontrol", {"configured": False, "reason": "System CLI is not installed", "user_access": "unknown"})
        if not headset.get("configured") and HEADSET.is_file():
            headset = probe_headset(HEADSET)
        if not headset.get("configured"):
            pending.append("headsetcontrol: no working CLI passes the verified API 1.6 contract: " + headset.get("reason", "probe failed"))
        if failure_error:
            status = "unknown"
        elif failure and failure["status"] == "failed" or checkpoint and checkpoint.get("status") == "failed":
            status = "failed"
        else:
            status = "pending" if pending else "running"
    elif query_error:
        status = "unknown"
        pending.append("native-package: " + query_error)
    elif engine_running:
        status = "failed" if failure and failure["status"] == "failed" else "pending"
        pending.append("private-engine: backend is still active although the native package is absent")
    else:
        status = "removed" if any(record and record.get("status") == "removed" for record in (state, checkpoint)) else "not-installed"
    return {
        "status": status, "package_installed": None if query_error else bool(package),
        "package_version": package["version"] if package else None, "package_state": package["state"] if package else "unknown" if query_error else "not-installed",
        "system_record": state, "native_record": checkpoint, "native_failure": failure, "native_failure_error": failure_error,
        "user_agent_running": agent_running, "private_engine_running": engine_running,
        "headsetcontrol": headset, "pending": list(dict.fromkeys(pending)), "notices": list(dict.fromkeys(notices)),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("install", "repair", "uninstall", "status"), nargs="?", default="install")
    parser.add_argument("--bundle", type=Path, default=HERE)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--fixture", type=Path, help="os-release/facts fixture; allowed only with --dry-run")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--yes", action="store_true", help="Confirm the overall operation, NOT disruptive backend choices")
    parser.add_argument("--without-audio", action="store_true")
    parser.add_argument("--without-openrazer", action="store_true")
    parser.add_argument("--purge", action="store_true", help="Remove installer-owned state/repos on uninstall; never external configs or user profiles")
    for flag in ("audio-switch", "session-manager", "ratbag", "repository", "unsigned-dkms"):
        parser.add_argument("--consent-" + flag, action="store_true")
    args = parser.parse_args()
    try:
        if args.fixture and not args.dry_run:
            raise PackagingError("Fixtures cannot control a real installation")
        if not args.dry_run and (sys.platform != "linux" or os.geteuid() == 0):
            raise PackagingError("Run on Linux as the normal user. Host Windows installations and root entry are forbidden")
        manifest = None
        manifest_hash = None
        if (args.bundle / "manifest.json").is_file():
            manifest, manifest_hash = verify_bundle(args.bundle)
        elif not args.dry_run and args.action != "status":
            raise PackagingError("A complete, verified bundle is required, not the repository sources or an AppImage")
        if args.fixture:
            fixture = json_file(args.fixture)
            release = parse_os_release(fixture["os_release"])
            facts = fixture["facts"]
        else:
            if sys.platform != "linux":
                raise PackagingError("Use --dry-run --fixture on non-Linux hosts")
            release = parse_os_release(regular_bytes(Path("/etc/os-release").resolve(), 65536).decode())
            target = target_for(release)
            facts = facts_for(target)
        target = target_for(release)
        preflight(target, facts, manifest if args.action != "status" else None)
        if args.action == "status" and not args.dry_run:
            observed = installation_status(target, facts)
            observed.update(target=target["id"], host_os_release=release, compatible_host=release.get("ID") != target["os_id"])
            print(json.dumps(observed, indent=2, sort_keys=True))
            return 0 if observed["status"] == "running" else 2
        choices = choices_for(args, target, facts)
        package = str((args.bundle / manifest["package"]["file"]).resolve()) if manifest else f"peripheral-hub-0.1.0.{target['format']}"
        plan = system_plan(target, facts, choices, args.action, package)
        plan.update(host_os_release=release, compatible_host=release.get("ID") != target["os_id"])
        plan["user_commands"] = user_commands(plan, facts, choices)
        plan["sources"] = load_targets()["sources"]
        if args.json:
            print(json.dumps(plan, indent=2, sort_keys=True))
        else:
            print_plan(plan, manifest, facts)
        if args.dry_run:
            return 0
        if not args.yes and not ask("Proceed with this explicit native transaction?", False, args):
            raise PackagingError("Operation declined; no administrative authentication requested")
        with tempfile.TemporaryDirectory(prefix="peripheral-hub-install-") as temporary:
            work = Path(temporary)
            key = None
            key_checksum = None
            if plan["repository"]:
                data = https_bytes(plan["repository"]["key_url"], 128 * 1024)
                expected = load_targets()["repository_keys"][plan["repository"]["key"]]
                if pgp_fingerprint(data) != expected:
                    raise PackagingError("Official repository key fingerprint mismatch; no system changes")
                key = work / "repository.asc"
                key.write_bytes(data)
                key_checksum = digest(data)
            request = {"schema": 1, "uid": os.geteuid(), "action": args.action, "target": target["id"], "choices": choices, "manifest_sha256": manifest_hash, "purge": args.purge, "yes": args.yes, "repository_key_sha256": key_checksum}
            write_json(work / "request.json", request, 0o600)
            if args.action == "uninstall":
                for command in plan["user_commands"]:
                    if not Path(command["argv"][0]).exists() and command["argv"][0].startswith("/"):
                        continue
                    result = run(command["argv"], check=False, capture=False)
                    if result.returncode:
                        raise PackagingError("User recovery/agent shutdown failed; private system backend was not removed")
            auth = ["sudo", "--"] if shutil.which("sudo") else ["pkexec"] if shutil.which("pkexec") else None
            if not auth:
                raise PackagingError("Install requires sudo or Polkit pkexec; no root shell or automatic fallback")
            command = [*auth, "/usr/bin/python3", "-I", "-c", ROOT_BOOTSTRAP, str(args.bundle.resolve()), str(work / "request.json"), manifest_hash]
            if key:
                command.append(str(key))
            result = run(command, capture=False, timeout=3600, check=False)
            state = json_file(STATE) if STATE.exists() else {"status": "failed", "pending": ["System transaction failed before recording state"]}
            pending = list(dict.fromkeys([*plan["pending"], *state.get("pending", [])]))
            headset = None
            failure, failure_error = None, None
            if args.action != "uninstall" and state.get("package_installed"):
                try:
                    failure = native_failure(NATIVE_FAILURE)
                except PackagingError as error:
                    failure_error = str(error)
                if failure or failure_error:
                    pending.append("native-package: " + (failure_error or failure["error"] or "configure has not completed"))
            failed = bool(result.returncode or state.get("status") == "failed" or failure_error or failure and failure["status"] == "failed")
            if args.action != "uninstall" and not failed and not failure and state.get("package_installed"):
                if choices.get("openrazer") and "plugdev" not in facts.get("groups", []) and not any(item.startswith("openrazer:") for item in plan["pending"]):
                    pending.append("openrazer: pending logout/login; plugdev is not effective in this session")
                if not facts.get("session"):
                    pending.append("user-services: pending desktop login; never execute systemctl --user as root")
                else:
                    for operation in user_commands(plan, facts, choices):
                        try:
                            if "unit" in operation:
                                activate_user_unit(operation["unit"])
                            else:
                                run(operation["argv"], capture=False)
                        except PackagingError as error:
                            pending.append(str(error))
                cli = shutil.which("headsetcontrol")
                headset = probe_headset(cli) if cli else {"configured": False}
                if not headset.get("configured") and HEADSET.is_file():
                    headset = probe_headset(HEADSET)
                if not headset.get("configured"):
                    pending.append("headsetcontrol: no working CLI for verified API 1.6: " + headset.get("reason", "probe failed"))
            summary = {"status": "failed" if failed else "pending" if pending else "removed" if args.action == "uninstall" else "running", "pending": pending, "notices": state.get("notices", []), "headsetcontrol": headset, "system": state, "native_failure": failure}
            state_home = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
            if not state_home.is_absolute():
                raise PackagingError("XDG_STATE_HOME must be absolute")
            write_json(state_home / APP / "installer-status.json", summary, 0o600)
            print(json.dumps(summary, indent=2, sort_keys=True))
            return 1 if failed else 2 if pending else 0
    except (PackagingError, OSError, ValueError) as error:
        print(f"Installer error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
