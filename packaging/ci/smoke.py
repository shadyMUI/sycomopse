#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native lifecycle in delegated rootless Podman or a concretely verified disposable VM."""

import argparse
import json
import os
import re
import stat
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import AGENT, APP, ENGINE, PackagingError, parse_os_release, probe_headset, regular_bytes, run, target_for, verify_bundle
from build import update_validation
from ci.gates import require_native_preconditions, require_unpromoted


def verify_isolation(uid_map, devices=Path("/dev")):
    """Validate the private device tree and return UID mappings as JSON arrays."""
    try:
        mappings = [[int(field) for field in line.split()] for line in uid_map.splitlines() if line.strip()]
    except ValueError as error:
        raise PackagingError("Cannot verify the smoke user namespace") from error
    if not mappings or any(len(item) != 3 or item[0] < 0 or item[1] <= 0 or item[2] <= 0 for item in mappings) or not any(item[0] == 0 for item in mappings):
        raise PackagingError("Smoke root must map to an unprivileged host UID, never host root")
    for name in ("input", "uinput", "snd", "kvm", "mem", "kmem", "port"):
        if (devices / name).exists():
            raise PackagingError("Host/hardware devices are forbidden in native smoke: " + name)
    allowed = {(1, 3), (1, 5), (1, 7), (1, 8), (1, 9), (5, 0), (5, 2)}
    for path in devices.rglob("*"):
        info = path.lstat()
        if stat.S_ISBLK(info.st_mode):
            raise PackagingError("Block devices are forbidden in native smoke")
        if stat.S_ISCHR(info.st_mode) and (os.major(info.st_rdev), os.minor(info.st_rdev)) not in allowed and os.major(info.st_rdev) != 136:
            raise PackagingError("Unexpected character device in native smoke: " + str(path))
    return mappings


def verify_service_state(enabled, active, stage):
    enablement = run(["systemctl", "is-enabled", ENGINE], check=False)
    activity = run(["systemctl", "is-active", ENGINE], check=False)
    if enablement.stdout.strip() not in ("enabled", "disabled") or activity.stdout.strip() not in ("active", "inactive"):
        raise PackagingError(stage + ": private service is missing or has an unexpected systemd state")
    actual_enabled = enablement.returncode == 0
    actual_active = activity.returncode == 0
    if actual_enabled != enabled or actual_active != active:
        raise PackagingError(f"{stage}: expected enabled={enabled}, active={active}; observed enabled={actual_enabled}, active={actual_active}")


def main(vm_contract=None, observer=None):
    if vm_contract is None:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("--bundle", required=True, type=Path)
        parser.add_argument("--builder-uid", required=True, type=int)
        args = parser.parse_args()
        if sys.platform != "linux" or os.geteuid() != 0 or os.environ.get("PH_CI_SMOKE") != "1" or not any(Path(name).is_file() for name in ("/.dockerenv", "/run/.containerenv", "/.containerenv")):
            raise PackagingError("Smoke tests may install packages only in the explicitly isolated Linux CI container")
        if os.environ.get("PH_CI_SMOKE_EXECUTOR") != "rootless-podman" or not re.fullmatch(r"sha256:[a-f0-9]{64}", os.environ.get("PH_CI_SMOKE_IMAGE_ID", "")):
            raise PackagingError("Native smoke requires identified rootless Podman isolation; Docker has no privileged fallback")
        mappings = verify_isolation(os.environ.get("PH_CI_SMOKE_HOST_UID_MAP", ""))
        container_map = regular_bytes("/proc/self/uid_map", 65536).decode()
        if regular_bytes("/proc/1/comm", 128, owner=0).decode().strip() != "systemd":
            raise PackagingError("Native service smoke requires systemd as PID 1")
        import pwd
        if args.builder_uid != 1000 or pwd.getpwnam("builder").pw_uid != args.builder_uid:
            raise PackagingError("Rootless smoke requires the separate builder session user UID 1000")
        bundle, uid, username = args.bundle.resolve(), args.builder_uid, "builder"
    else:
        from ci.vm_guest import INPUT, require_guest
        require_guest(vm_contract)
        bundle, uid, username = INPUT / "bundle", 1000, "phci"
    manifest, original_sha = verify_bundle(bundle, vm_contract["manifest_sha256"] if vm_contract else None)
    require_native_preconditions(manifest)
    require_unpromoted(manifest)
    if vm_contract is None and bundle.stat().st_uid != 0:
        raise PackagingError("Native smoke requires the bundle owner to map to container root")
    release = parse_os_release(regular_bytes(Path("/etc/os-release").resolve(), 65536, owner=0).decode())
    target = target_for(release, allow_compatible=False)
    if target["id"] != manifest["target"]["id"]:
        raise PackagingError("Smoke target does not match the actual artifact ABI")
    package = str(bundle / manifest["package"]["file"])
    commands = {
        "apt": ["apt-get", "install", "--yes", "--no-install-recommends", package],
        "pacman": ["pacman", "-U", "--noconfirm", "--", package],
        "dnf": ["dnf", "install", "--assumeyes", package],
        "zypper": ["zypper", "--non-interactive", "install", package],
    }
    reinstall = {
        "apt": ["apt-get", "install", "--reinstall", "--yes", "--no-install-recommends", package],
        "pacman": commands["pacman"],
        "dnf": ["dnf", "reinstall", "--assumeyes", package],
        "zypper": ["zypper", "--non-interactive", "install", "--force", package],
    }
    steps = []
    transactions = []
    def transact(argv, timeout=1200):
        result = run(argv, capture=False, timeout=timeout)
        record = {"argv": list(argv), "returncode": result.returncode}
        transactions.append(record)
        return record
    def checkpoint(name):
        steps.append(name)
        if observer is not None:
            observer(name, list(transactions))
        transactions.clear()
    # Invalidate an earlier run before any new transaction, and never promote on partial success.
    manifest["validation"].update(native_smoke="not-run", native_smoke_steps=[])
    manifest["validation"].pop("native_smoke_evidence", None)
    transact(commands[target["manager"]])
    checkpoint("native-install")
    run(["/usr/lib/peripheral-hub/remapper-service", "--check-imports"], capture=False)
    # No installer script or manual system-service activation may make this pass.
    run(["systemctl", "is-active", ENGINE])
    checkpoint("native-hooks-autostart")
    headset = probe_headset("/usr/lib/peripheral-hub/headsetcontrol")
    if not headset["configured"]:
        raise PackagingError("Installed private HeadsetControl contract failed: " + headset["reason"])
    checkpoint("installed-headsetcontrol-api")
    version = run(["busctl", "--system", "call", "io.peripheralhub.InputRemapper", "/io/peripheralhub/InputRemapper", "io.peripheralhub.InputRemapper1", "GetVersion"]).stdout.strip()
    if version != "u 1":
        raise PackagingError("Installed private DBus bridge protocol did not return u 1")
    checkpoint("private-service-and-dbus")
    run(["systemctl", "start", f"user@{uid}.service"], capture=False)
    user = ["runuser", "-u", username, "--", "env", f"XDG_RUNTIME_DIR=/run/user/{uid}", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"]
    # Execute the same non-root operation as the installed XDG autostart entry.
    autostart = regular_bytes("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop", 4096).decode()
    if "Exec=systemctl --user start peripheral-hub-remapping-agent.service\n" not in autostart:
        raise PackagingError("Native package lacks its user-session autostart entry")
    run([*user, "systemctl", "--user", "start", AGENT], capture=False)
    run([*user, "systemctl", "--user", "is-active", AGENT])
    run([*user, "/usr/bin/peripheral-hub-remapping-agent", "--disable-all"], capture=False)
    checkpoint("normal-user-agent-and-recovery")
    autostart_path = Path("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop")
    edited_autostart = autostart + "Hidden=true\n"
    autostart_path.write_text(edited_autostart, encoding="utf-8")
    transact(reinstall[target["manager"]])
    run(["systemctl", "is-active", ENGINE])
    if regular_bytes(autostart_path, 4096).decode() != edited_autostart:
        raise PackagingError("Upgrade overwrote the administrator's Hidden=true autostart setting")
    checkpoint("same-version-upgrade-hooks")
    checkpoint("upgrade-preserves-autostart-config")
    # Deliberately remove only a package-owned recovery desktop, then verify repair restores it.
    recovery = Path("/usr/share/applications/io.peripheralhub.Recovery.desktop")
    recovery.unlink()
    transact(reinstall[target["manager"]])
    if not recovery.is_file():
        raise PackagingError("Native repair did not restore its own missing resource")
    if regular_bytes(autostart_path, 4096).decode() != edited_autostart:
        raise PackagingError("Native repair overwrote the administrator's autostart setting")
    checkpoint("repair-missing-owned-file")
    run(["systemctl", "disable", "--now", ENGINE], capture=False)
    transact(reinstall[target["manager"]])
    verify_service_state(False, False, "Upgrade with an administrator-disabled backend")
    checkpoint("upgrade-preserves-disabled-service")
    status_path = Path("/var/lib/peripheral-hub/native-status.json")
    for enabled in (False, True):
        if enabled:
            run(["systemctl", "enable", ENGINE], capture=False)
        status_path.unlink()
        transact(reinstall[target["manager"]])
        verify_service_state(enabled, False, "First upgrade without a native registry")
        checkpoint("legacy-upgrade-preserves-stopped-service" if enabled else "legacy-upgrade-preserves-disabled-service")
    remove = {
        "apt": ["apt-get", "remove", "--yes", APP], "pacman": ["pacman", "-R", "--noconfirm", "--", APP],
        "dnf": ["dnf", "remove", "--assumeyes", "--noautoremove", APP],
        "zypper": ["zypper", "--non-interactive", "remove", APP],
    }
    # These explicit state changes are test setup, after native-only startup passed.
    for enabled, active, label in ((False, False, "disabled"), (True, False, "stopped"), (True, True, "running")):
        run(["systemctl", "enable" if enabled else "disable", *(["--now"] if active else []), ENGINE], capture=False)
        if not active:
            run(["systemctl", "stop", ENGINE], capture=False)
        removal = transact(remove[target["manager"]], timeout=600)
        if any(Path(name).exists() for name in ("/usr/bin/peripheral-hub", "/usr/bin/peripheral-hub-remapping-agent", "/usr/bin/peripheral-hub-audio-worker", "/usr/lib/peripheral-hub/remapper-service")):
            raise PackagingError("Native uninstall left app/engine executable behind")
        if run(["systemctl", "is-active", ENGINE], check=False).returncode == 0:
            raise PackagingError("Native uninstall left the private backend running")
        removal["after_remove"] = {"private_payload_present": False, "backend_active": False}
        transact(commands[target["manager"]])
        verify_service_state(enabled, active, "Remove/reinstall with a previously " + label + " backend")
        checkpoint("remove-reinstall-preserves-" + label + "-service")
    checkpoint("uninstall-private-payload")
    checkpoint("native-reinstall-idempotence")
    if vm_contract is not None:
        return {"native_smoke": "passed", "steps": steps}
    manifest["validation"]["native_smoke"] = "passed"
    manifest["validation"]["native_smoke_steps"] = steps
    manifest["validation"]["native_smoke_evidence"] = {
        "executor": "rootless-podman", "scope": "isolated-systemd-package-lifecycle",
        "runtime_image_id": os.environ["PH_CI_SMOKE_IMAGE_ID"], "os_release": release,
        "uid_map": mappings, "uid_map_scope": "initial-host", "container_uid_map": container_map,
        "host_devices": "none", "user_manager": "created-for-test",
        "input_manifest_sha256": original_sha,
    }
    manifest["validation"].update(linux_hardware="not-run", graphical_login_reboot="not-run", audio_hardware="not-run", dkms_host_kernel="not-run")
    manifest["validation"]["native_smoke_limitations"] = "Actual package transactions and private service/user-manager checks only. Recording/fake engine endpoints and parser tests do not certify HID, DKMS, physical audio, graphical login or reboot. No host devices, kernel modules or session are mounted."
    native_validation = {key: value for key, value in manifest["validation"].items()
                         if key.startswith("native_smoke") or key in ("linux_hardware", "graphical_login_reboot", "audio_hardware", "dkms_host_kernel")}
    update_validation(bundle, original_sha, native_validation)
    print(json.dumps({"native_smoke": "passed", "target": target["id"], "steps": steps}, indent=2))


if __name__ == "__main__":
    main()
