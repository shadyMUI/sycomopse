#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native install/reinstall/repair/uninstall smoke in an isolated systemd container."""

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import AGENT, APP, ENGINE, PackagingError, parse_os_release, regular_bytes, run, target_for, verify_bundle
from build import write_manifest


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
    run(["systemctl", "daemon-reload"], capture=False)
    run(["systemctl", "enable", "--now", ENGINE], capture=False)
    run(["systemctl", "is-active", ENGINE])
    version = run(["busctl", "--system", "call", "io.peripheralhub.InputRemapper", "/io/peripheralhub/InputRemapper", "io.peripheralhub.InputRemapper1", "GetVersion"]).stdout.strip()
    if version != "u 1":
        raise PackagingError("Installed private DBus bridge protocol did not return u 1")
    steps.append("private-service-and-dbus")
    uid = args.builder_uid
    run(["systemctl", "start", f"user@{uid}.service"], capture=False)
    user = ["runuser", "-u", "builder", "--", "env", f"XDG_RUNTIME_DIR=/run/user/{uid}", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"]
    run([*user, "systemctl", "--user", "enable", "--now", AGENT], capture=False)
    run([*user, "systemctl", "--user", "is-active", AGENT])
    run([*user, "/usr/bin/peripheral-hub-remapping-agent", "--disable-all"], capture=False)
    steps.append("normal-user-agent-and-recovery")
    run(reinstall[target["manager"]], capture=False, timeout=1200)
    steps.append("same-version-upgrade-hooks")
    # Deliberately remove only a package-owned recovery desktop, then verify repair restores it.
    recovery = Path("/usr/share/applications/io.peripheralhub.Recovery.desktop")
    recovery.unlink()
    run(reinstall[target["manager"]], capture=False, timeout=1200)
    if not recovery.is_file():
        raise PackagingError("Native repair did not restore its own missing resource")
    steps.append("repair-missing-owned-file")
    run([*user, "systemctl", "--user", "disable", "--now", AGENT], capture=False)
    run(["systemctl", "disable", "--now", ENGINE], capture=False)
    remove = {
        "apt": ["apt-get", "remove", "--yes", APP], "pacman": ["pacman", "-R", "--noconfirm", "--", APP],
        "dnf": ["dnf", "remove", "--assumeyes", "--noautoremove", APP],
        "zypper": ["zypper", "--non-interactive", "remove", APP],
    }
    run(remove[target["manager"]], capture=False, timeout=600)
    if Path("/usr/bin/peripheral-hub").exists() or Path("/usr/lib/peripheral-hub/remapper-service").exists():
        raise PackagingError("Native uninstall left app/engine executable behind")
    steps.append("uninstall-private-payload")
    # Reinstall proves idempotence without starting/stopping any external ratbag/audio/Razer service.
    run(commands[target["manager"]], capture=False, timeout=1200)
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
