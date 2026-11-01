#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native package lifecycle: configure only package-owned services, never packages."""

import argparse
import os
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import ENGINE, PackagingError, json_file, run, write_json

STATUS = Path("/var/lib/peripheral-hub/native-status.json")
IMPORT_CHECK = "/usr/lib/peripheral-hub/remapper-service"
ENABLED_STATES = ("enabled", "enabled-runtime", "static", "indirect", "generated")


def configure(package_format, upgrade=False, runner=run, status=STATUS,
              systemd_running=None, available=shutil.which):
    if package_format not in ("deb", "rpm", "arch"):
        raise PackagingError("Unknown native package format")
    live = Path("/run/systemd/system").is_dir() if systemd_running is None else systemd_running
    previous = json_file(status) if status.exists() else None
    if previous is not None and previous.get("schema") != 1:
        raise PackagingError("Unknown native status schema; original file preserved")
    first_configuration = not upgrade and previous is None
    removal = previous.get("removal") if previous and previous.get("engine") == ENGINE else None
    removal = removal if isinstance(removal, dict) else {}
    restore_enabled = removal.get("enabled") is True and removal.get("disabled") is True
    restore_running = removal.get("enabled") is True and removal.get("running") is True and removal.get("stopped") is True
    scope = [] if live else ["--root=/"]
    if live:
        runner(["systemctl", "daemon-reload"])
    state = runner(["systemctl", *scope, "is-enabled", ENGINE], check=False).stdout.strip()
    active = live and runner(["systemctl", "is-active", ENGINE], check=False).returncode == 0
    record = dict(previous or {})
    # Only a successfully saved configuration consumes the removal checkpoint.
    record.pop("removal", None)
    record.update(schema=1, format=package_format, engine=ENGINE, pending=[])
    if state.startswith("masked"):
        record.update(status="masked", enabled=False, running=active)
        record["pending"].append("Private backend masked by administrator; mask preserved")
        write_json(status, record)
        return record
    if first_configuration or (restore_enabled and state == "disabled"):
        if package_format == "deb":
            if not available("deb-systemd-helper"):
                raise PackagingError("init-system-helpers is required by the native DEB")
            runner(["deb-systemd-helper", "enable", ENGINE])
        else:
            # Package preset may be overridden by an administrator's earlier rule.
            runner(["systemctl", *scope, "preset", ENGINE])
        state = runner(["systemctl", *scope, "is-enabled", ENGINE], check=False).stdout.strip()
    enabled = state in ENABLED_STATES
    record["enabled"] = enabled
    if not live:
        record.update(status="pending-boot", running=False)
        record["pending"].append("Service configured offline; starts when systemd boots" if enabled else "Private backend disabled offline; state preserved")
    elif not enabled:
        record.update(status="disabled", running=active)
        record["pending"].append("Private backend disabled by administrator or preset; state preserved")
    else:
        runner([IMPORT_CHECK, "--check-imports"], timeout=30)
        if package_format == "deb":
            if not available("deb-systemd-invoke"):
                raise PackagingError("init-system-helpers is required by the native DEB")
            # Honor policy-rc.d. An upgrade must not restart a deliberately stopped unit.
            if first_configuration or restore_running or active:
                runner(["deb-systemd-invoke", "restart" if active else "start", ENGINE], timeout=60)
        elif first_configuration or restore_running or active:
            runner(["systemctl", "start" if first_configuration or restore_running else "try-restart", ENGINE], timeout=60)
        active = runner(["systemctl", "is-active", ENGINE], check=False).returncode == 0
        record.update(status="running" if active else "pending", running=active)
        if not active:
            record["pending"].append("Native startup policy or stopped service: backend not active")
    write_json(status, record)
    return record


def remove(package_format, runner=run, systemd_running=None, available=shutil.which, status=STATUS):
    if package_format not in ("deb", "rpm", "arch"):
        raise PackagingError("Unknown native package format")
    live = Path("/run/systemd/system").is_dir() if systemd_running is None else systemd_running
    previous = json_file(status) if status.exists() else None
    if previous is not None and previous.get("schema") != 1:
        raise PackagingError("Unknown native status schema; original file preserved")
    scope = [] if live else ["--root=/"]
    state = runner(["systemctl", *scope, "is-enabled", ENGINE], check=False).stdout.strip()
    enabled_before = state in ENABLED_STATES
    active = runner(["systemctl", "is-active", ENGINE], check=False).returncode == 0 if live else None
    record = dict(previous or {})
    removal = record.get("removal") if record.get("engine") == ENGINE else None
    # Keep the original observation across retries of an interrupted removal.
    removal = dict(removal) if isinstance(removal, dict) else {
        "enabled": enabled_before, "running": active, "disabled": False, "stopped": False,
    }
    record.update(schema=1, format=package_format, engine=ENGINE, status="removing",
                  enabled=enabled_before, running=bool(active), pending=[], removal=removal)
    write_json(status, record)
    errors = record["pending"]
    if not state.startswith("masked"):
        command = ["deb-systemd-helper", "disable", ENGINE] if package_format == "deb" and available("deb-systemd-helper") else ["systemctl", *scope, "disable", ENGINE]
        response = runner(command, check=False)
        state = runner(["systemctl", *scope, "is-enabled", ENGINE], check=False).stdout.strip()
        record["enabled"] = state in ENABLED_STATES
        if response.returncode or record["enabled"]:
            errors.append("Private backend disable failed; inspect systemd before retrying removal")
        elif enabled_before and removal.get("enabled") is True:
            removal["disabled"] = True
    write_json(status, record)
    if live:
        response = runner(["systemctl", "stop", ENGINE], check=False, timeout=60)
        record["running"] = runner(["systemctl", "is-active", ENGINE], check=False).returncode == 0
        if response.returncode or record["running"]:
            errors.append("Private backend stop failed; inspect systemd before retrying removal")
        elif active is True and removal.get("running") is True:
            removal["stopped"] = True
    failed = bool(errors)
    if not live:
        record["pending"].append("Private backend removed offline; previous runtime state is unknown")
    record["status"] = "remove-failed" if failed else "removed"
    write_json(status, record)
    if record["status"] == "remove-failed":
        raise PackagingError("; ".join(errors))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("configure", "remove", "status"))
    parser.add_argument("--format", choices=("deb", "rpm", "arch"))
    parser.add_argument("--upgrade", action="store_true")
    args = parser.parse_args()
    try:
        if args.action == "status":
            import json
            print(json.dumps(json_file(STATUS), indent=2))
            return 0
        if sys.platform != "linux" or os.geteuid() != 0 or not args.format:
            raise PackagingError("Native lifecycle requires a root package-manager hook on Linux")
        if args.action == "configure":
            result = configure(args.format, args.upgrade)
            for message in result["pending"]:
                print("Peripheral Hub: " + message, file=sys.stderr)
        else:
            remove(args.format)
    except (PackagingError, OSError, ValueError) as error:
        print("Peripheral Hub native configuration failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
