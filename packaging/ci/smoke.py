#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native install/reinstall/repair/uninstall smoke in an isolated systemd container."""

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import AGENT, APP, ENGINE, PackagingError, parse_os_release, probe_headset, regular_bytes, run, target_for, verify_bundle
from build import write_manifest


def verify_service_state(enabled, active, stage):
    enablement = run(["systemctl", "is-enabled", ENGINE], check=False)
    activity = run(["systemctl", "is-active", ENGINE], check=False)
    if enablement.stdout.strip() not in ("enabled", "disabled") or activity.stdout.strip() not in ("active", "inactive"):
        raise PackagingError(stage + ": private service is missing or has an unexpected systemd state")
    actual_enabled = enablement.returncode == 0
    actual_active = activity.returncode == 0
    if actual_enabled != enabled or actual_active != active:
        raise PackagingError(f"{stage}: expected enabled={enabled}, active={active}; observed enabled={actual_enabled}, active={actual_active}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--builder-uid", required=True, type=int)
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() != 0 or os.environ.get("PH_CI_SMOKE") != "1" or not Path("/.dockerenv").exists():
        raise PackagingError("Smoke tests may install packages only in the explicitly isolated Linux CI container")
    if regular_bytes("/proc/1/comm", 128).decode().strip() != "systemd":
        raise PackagingError("Native service smoke requires systemd as PID 1")
    bundle = args.bundle.resolve()
    manifest, _ = verify_bundle(bundle)
    target = target_for(parse_os_release(regular_bytes("/etc/os-release", 65536).decode()))
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
    run(commands[target["manager"]], capture=False, timeout=1200)
    steps.append("native-install")
    run(["/usr/lib/peripheral-hub/remapper-service", "--check-imports"], capture=False)
    # No installer script or manual system-service activation may make this pass.
    run(["systemctl", "is-active", ENGINE])
    steps.append("native-hooks-autostart")
    headset = probe_headset("/usr/lib/peripheral-hub/headsetcontrol")
    if not headset["configured"]:
        raise PackagingError("Installed private HeadsetControl contract failed: " + headset["reason"])
    steps.append("installed-headsetcontrol-api")
    version = run(["busctl", "--system", "call", "io.peripheralhub.InputRemapper", "/io/peripheralhub/InputRemapper", "io.peripheralhub.InputRemapper1", "GetVersion"]).stdout.strip()
    if version != "u 1":
        raise PackagingError("Installed private DBus bridge protocol did not return u 1")
    steps.append("private-service-and-dbus")
    uid = args.builder_uid
    run(["systemctl", "start", f"user@{uid}.service"], capture=False)
    user = ["runuser", "-u", "builder", "--", "env", f"XDG_RUNTIME_DIR=/run/user/{uid}", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"]
    # Execute the same non-root operation as the installed XDG autostart entry.
    autostart = regular_bytes("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop", 4096).decode()
    if "Exec=systemctl --user start peripheral-hub-remapping-agent.service\n" not in autostart:
        raise PackagingError("Native package lacks its user-session autostart entry")
    run([*user, "systemctl", "--user", "start", AGENT], capture=False)
    run([*user, "systemctl", "--user", "is-active", AGENT])
    run([*user, "/usr/bin/peripheral-hub-remapping-agent", "--disable-all"], capture=False)
    steps.append("normal-user-agent-and-recovery")
    autostart_path = Path("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop")
    edited_autostart = autostart + "Hidden=true\n"
    autostart_path.write_text(edited_autostart, encoding="utf-8")
    run(reinstall[target["manager"]], capture=False, timeout=1200)
    run(["systemctl", "is-active", ENGINE])
    if regular_bytes(autostart_path, 4096).decode() != edited_autostart:
        raise PackagingError("Upgrade overwrote the administrator's Hidden=true autostart setting")
    steps.append("same-version-upgrade-hooks")
    steps.append("upgrade-preserves-autostart-config")
    # Deliberately remove only a package-owned recovery desktop, then verify repair restores it.
    recovery = Path("/usr/share/applications/io.peripheralhub.Recovery.desktop")
    recovery.unlink()
    run(reinstall[target["manager"]], capture=False, timeout=1200)
    if not recovery.is_file():
        raise PackagingError("Native repair did not restore its own missing resource")
    if regular_bytes(autostart_path, 4096).decode() != edited_autostart:
        raise PackagingError("Native repair overwrote the administrator's autostart setting")
    steps.append("repair-missing-owned-file")
    run(["systemctl", "disable", "--now", ENGINE], capture=False)
    run(reinstall[target["manager"]], capture=False, timeout=1200)
    verify_service_state(False, False, "Upgrade with an administrator-disabled backend")
    steps.append("upgrade-preserves-disabled-service")
    status_path = Path("/var/lib/peripheral-hub/native-status.json")
    for enabled in (False, True):
        if enabled:
            run(["systemctl", "enable", ENGINE], capture=False)
        status_path.unlink()
        run(reinstall[target["manager"]], capture=False, timeout=1200)
        verify_service_state(enabled, False, "First upgrade without a native registry")
        steps.append("legacy-upgrade-preserves-stopped-service" if enabled else "legacy-upgrade-preserves-disabled-service")
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
        run(remove[target["manager"]], capture=False, timeout=600)
        if Path("/usr/bin/peripheral-hub").exists() or Path("/usr/lib/peripheral-hub/remapper-service").exists():
            raise PackagingError("Native uninstall left app/engine executable behind")
        if run(["systemctl", "is-active", ENGINE], check=False).returncode == 0:
            raise PackagingError("Native uninstall left the private backend running")
        run(commands[target["manager"]], capture=False, timeout=1200)
        verify_service_state(enabled, active, "Remove/reinstall with a previously " + label + " backend")
        steps.append("remove-reinstall-preserves-" + label + "-service")
    steps.append("uninstall-private-payload")
    steps.append("native-reinstall-idempotence")
    manifest["validation"]["native_smoke"] = "passed"
    manifest["validation"]["native_smoke_steps"] = steps
    manifest["validation"]["native_smoke_limitations"] = "No physical HID, graphical login, audio replacement or host-kernel DKMS test in this container"
    write_manifest(bundle, manifest)
    for path in (bundle / "manifest.json", bundle / "manifest.sha256"):
        os.chown(path, uid, -1)
    print(json.dumps({"native_smoke": "passed", "target": target["id"], "steps": steps}, indent=2))


if __name__ == "__main__":
    main()
