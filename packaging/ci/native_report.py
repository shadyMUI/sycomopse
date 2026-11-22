# SPDX-License-Identifier: MIT
"""Exact guest command/state policy for native package lifecycle evidence."""

import re

from common import AGENT, APP, ENGINE, PackagingError


def checkpoint_commands():
    return {
        "package": ["dpkg-query", "-W", "-f=${db:Status-Abbrev} ${Version}\n", APP],
        "backend": ["systemctl", "show", ENGINE, "--property=LoadState,UnitFileState,ActiveState,SubState,MainPID"],
        "user_manager": ["systemctl", "show", "user@1000.service", "--property=ActiveState,SubState,MainPID"],
        "agent": ["runuser", "-u", "phci", "--", "env", "XDG_RUNTIME_DIR=/run/user/1000",
                  "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus", "systemctl", "--user", "is-active", AGENT],
    }


def unit_fields(text, keys):
    fields = {}
    for line in text.splitlines():
        if "=" not in line:
            raise PackagingError("Malformed systemd unit evidence")
        key, value = line.split("=", 1)
        if key in fields:
            raise PackagingError("Duplicate systemd unit evidence")
        fields[key] = value
    if set(fields) != set(keys):
        raise PackagingError("Missing or foreign systemd unit evidence fields")
    return fields


def validate_checkpoints(report, contract, step_names):
    observations = report.get("observations")
    if report.get("ready") is not True or report.get("error") is not None or report.get("native_smoke") != "passed" or report.get("steps") != list(step_names) or not isinstance(observations, list) or len(observations) != len(step_names):
        raise PackagingError("Native VM lifecycle has not completed successfully")
    commands = checkpoint_commands()
    package_path = "/mnt/phci/bundle/" + contract["package"]["file"]
    install = ["apt-get", "install", "--yes", "--no-install-recommends", package_path]
    reinstall = ["apt-get", "install", "--reinstall", "--yes", "--no-install-recommends", package_path]
    remove = ["apt-get", "remove", "--yes", APP]
    initial_autostart = observations[0].get("autostart")
    edited_autostart = observations[5].get("autostart")
    if initial_autostart == edited_autostart:
        raise PackagingError("Native upgrade evidence lacks the administrator autostart edit")
    for index, (name, observed) in enumerate(zip(step_names, observations)):
        if not isinstance(observed, dict) or observed.get("checkpoint") != name or not isinstance(observed.get("commands"), dict) or set(observed["commands"]) != set(commands):
            raise PackagingError("Native VM checkpoint evidence is missing or reordered")
        for label, argv in commands.items():
            result = observed["commands"][label]
            if not isinstance(result, dict) or result.get("argv") != argv or type(result.get("returncode")) is not int or not isinstance(result.get("stdout"), str) or not isinstance(result.get("stderr"), str) or len(result["stdout"]) > 8192 or len(result["stderr"]) > 4096:
                raise PackagingError("Native VM command evidence does not match its precise allowlist")
            if label != "agent" and result["returncode"] != 0:
                raise PackagingError("Native VM package/systemd observation failed")
        package = observed["commands"]["package"]["stdout"].split()
        if package != ["ii", contract["package"]["version"]]:
            raise PackagingError("Native VM package is not installed at the expected version")
        enabled = index not in (8, 9, 11)
        active = index not in (8, 9, 10, 11, 12)
        backend = unit_fields(observed["commands"]["backend"]["stdout"], ("LoadState", "UnitFileState", "ActiveState", "SubState", "MainPID"))
        if backend["LoadState"] != "loaded" or backend["UnitFileState"] != ("enabled" if enabled else "disabled") or backend["ActiveState"] != ("active" if active else "inactive") or backend["SubState"] != ("running" if active else "dead") or not backend["MainPID"].isdigit() or (int(backend["MainPID"]) > 0) != active:
            raise PackagingError("Native VM backend state differs from the deliberately established step baseline")
        manager = unit_fields(observed["commands"]["user_manager"]["stdout"], ("ActiveState", "SubState", "MainPID"))
        if manager["ActiveState"] != "active" or manager["SubState"] != "running" or not manager["MainPID"].isdigit() or int(manager["MainPID"]) <= 0:
            raise PackagingError("Native VM user manager is not actually running")
        agent = observed["commands"]["agent"]
        state = agent["stdout"].strip()
        if (agent["returncode"], state) not in ((0, "active"), (3, "inactive"), (4, "inactive")) or index < 4 and state != "inactive" or index == 4 and state != "active":
            raise PackagingError("Native VM user-agent return code/state contradicts its lifecycle baseline")
        for field in ("autostart", "native_registry"):
            if not isinstance(observed.get(field), str) or not re.fullmatch(r"[a-f0-9]{64}", observed[field]):
                raise PackagingError("Native VM lacks owned autostart/registry evidence")
        if observed["autostart"] != (initial_autostart if index < 5 else edited_autostart):
            raise PackagingError("Native VM did not preserve the administrator's autostart configuration")
        if observed.get("payload_present") is not True or observed.get("recovery_present") is not True or observed.get("native_failure_present") is not False:
            raise PackagingError("Native VM package/repair payload evidence is missing")
        expected = [install] if index == 0 else [reinstall] if index in (5, 7, 8, 9, 10) else [remove, install] if index in (11, 12, 13) else []
        transactions = observed.get("transactions")
        if not isinstance(transactions, list) or len(transactions) != len(expected):
            raise PackagingError("Native VM lacks the exact install/repair/remove transaction sequence")
        for transaction, argv in zip(transactions, expected):
            if not isinstance(transaction, dict) or transaction.get("argv") != argv or type(transaction.get("returncode")) is not int or transaction["returncode"] != 0:
                raise PackagingError("Native VM package transaction failed or used a foreign command")
            if argv == remove and transaction.get("after_remove") != {"private_payload_present": False, "backend_active": False}:
                raise PackagingError("Native VM uninstall lacks its actual absence/stopped postcondition")
