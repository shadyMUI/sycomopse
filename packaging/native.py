#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native package lifecycle: configure only package-owned services, never packages."""

import argparse
import os
import shutil
import stat
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import APP, ENGINE, NATIVE_FAILURE, NATIVE_STATE, PackagingError, build_target, json_file, native_failure, parse_os_release, regular_bytes, run, target_for, write_json

STATUS = NATIVE_STATE
BUILD_TARGET = Path(__file__).resolve().with_name("build-target.json")
IMPORT_CHECK = "/usr/lib/peripheral-hub/remapper-service"
ENABLED_STATES = ("enabled", "enabled-runtime", "static", "indirect", "generated")


def configure_checkpoint(package_format, status=STATUS, phase="configuring", error="", owner=0):
    if sys.platform != "linux" or os.geteuid() != 0:
        return
    if package_format not in ("deb", "rpm", "arch") or phase not in ("configuring", "failed", "complete"):
        raise PackagingError("Invalid native configuration checkpoint request")
    path = Path(status).with_name(NATIVE_FAILURE.name)
    previous = native_failure(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    directory = path.parent.lstat()
    if not stat.S_ISDIR(directory.st_mode) or directory.st_uid != owner or directory.st_mode & 0o022:
        raise PackagingError("Unsafe native state directory; checkpoint preserved")
    if phase == "complete":
        if previous:
            path.unlink()
    elif phase == "configuring" and previous:
        return
    else:
        write_json(path, {"schema": 1, "app": APP, "engine": ENGINE, "format": package_format, "status": phase, "error": (str(error) or "Native configuration failed")[:4000] if phase == "failed" else ""})
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        # Persist the in-flight/error entry, or the successful native-status save
        # and failure removal in this same directory, before reporting success.
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def native_state(status, package_format=None):
    try:
        info = Path(status).lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o022:
        raise PackagingError("Unsafe native status; original file preserved")
    previous = json_file(status)
    if (type(previous.get("schema")) is not int or previous["schema"] != 1
            or previous.get("engine", ENGINE) != ENGINE or previous.get("app", APP) != APP
            or "format" in previous and previous["format"] not in ("deb", "rpm", "arch")
            or package_format is not None and previous.get("format", package_format) != package_format):
        raise PackagingError("Unknown native status schema/identity; original file preserved")
    return previous


def configure(package_format, upgrade=False, runner=run, status=STATUS,
              systemd_running=None, available=shutil.which, host=None):
    try:
        if package_format not in ("deb", "rpm", "arch"):
            raise PackagingError("Unknown native package format")
        configure_checkpoint(package_format, status)
        live = Path("/run/systemd/system").is_dir() if systemd_running is None else systemd_running
        previous = native_state(status, package_format)
        # An old PID, a mask or an offline install does not prove the new payload
        # imports. This read-only check precedes all service/preset mutations.
        runner([IMPORT_CHECK, "--check-imports"], timeout=30)
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
        record.update(schema=1, format=package_format, engine=ENGINE, pending=[], imports_verified=True)
        if host is not None:
            record["host"] = host
        if state.startswith("masked"):
            record.update(status="masked", enabled=False, running=active)
            record["pending"].append("Private backend masked by administrator; mask preserved")
            write_json(status, record)
            return record
        if first_configuration or (restore_enabled and state == "disabled"):
            if package_format == "deb":
                if first_configuration:
                    if not available("deb-systemd-helper"):
                        raise PackagingError("init-system-helpers is required by the native DEB")
                    runner(["deb-systemd-helper", "enable", ENGINE])
                else:
                    # The Debian helper preserves removed links as an admin
                    # choice. Here the checkpoint proves our removal did it.
                    runner(["systemctl", *scope, "enable", ENGINE])
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
    except (PackagingError, OSError, ValueError) as error:
        try:
            configure_checkpoint(package_format, status, "failed", error)
        except (PackagingError, OSError, ValueError) as persistence_error:
            raise PackagingError(f"{error}; native failure checkpoint could not be saved: {persistence_error}") from error
        raise


def remove(package_format, runner=run, systemd_running=None, available=shutil.which, status=STATUS):
    if package_format not in ("deb", "rpm", "arch"):
        raise PackagingError("Unknown native package format")
    live = Path("/run/systemd/system").is_dir() if systemd_running is None else systemd_running
    previous = native_state(status, package_format)
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
    authorized_configure = False
    try:
        if args.action == "status":
            import json
            record = native_state(STATUS) or {"status": "not-configured", "pending": []}
            failure = native_failure(STATUS.with_name(NATIVE_FAILURE.name))
            if failure:
                record.update(native_status=record.get("status"), native_failure=failure, status=failure["status"])
            print(json.dumps(record, indent=2))
            return 0
        if sys.platform != "linux" or os.geteuid() != 0 or not args.format:
            raise PackagingError("Native lifecycle requires a root package-manager hook on Linux")
        if args.action == "configure":
            authorized_configure = True
            configure_checkpoint(args.format, STATUS)
            provenance = json_file(BUILD_TARGET)
            if type(provenance.get("schema")) is not int or provenance["schema"] != 1:
                raise PackagingError("Unknown native build provenance schema")
            built = build_target(provenance, args.format)
            release = parse_os_release(regular_bytes(Path("/etc/os-release").resolve(), 65536).decode())
            target = target_for(release)
            if target["id"] != built["id"]:
                raise PackagingError("Native payload was built for a different distribution/version")
            result = configure(args.format, args.upgrade, status=STATUS, host={"target": target["id"], "os_release": release, "build_target": provenance})
            if result.get("imports_verified") is not True:
                raise PackagingError("Native payload imports were not verified")
            configure_checkpoint(args.format, STATUS, "complete")
            for message in result["pending"]:
                print("Sycomopse: " + message, file=sys.stderr)
        else:
            remove(args.format)
    except (PackagingError, OSError, ValueError) as error:
        if authorized_configure:
            try:
                configure_checkpoint(args.format, STATUS, "failed", error)
            except (PackagingError, OSError, ValueError) as persistence_error:
                print("Native failure checkpoint preserved/unwritable: " + str(persistence_error), file=sys.stderr)
        print("Sycomopse native configuration failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
