#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Guest-only entrypoint: verify read-only ISO, run native lifecycle, emit bound serial evidence."""

import base64
import json
import os
import platform
import sys
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import AGENT, APP, ENGINE, PackagingError, digest, json_file, parse_os_release, regular_bytes, run, verify_bundle
from ci.vm import MAX_REPORT, canonical, checked_sha, file_sha, require_software, verify_contract
from ci.native_report import checkpoint_commands

INPUT = Path("/mnt/phci")


def require_guest(contract):
    verify_contract(contract)
    if sys.platform != "linux" or os.geteuid() != 0 or platform.machine() != "x86_64":
        raise PackagingError("Native VM operations require guest Linux x86_64 root")
    if any(Path(name).exists() for name in ("/.dockerenv", "/run/.containerenv", "/.containerenv")):
        raise PackagingError("The native VM entrypoint refuses OCI/host-container execution")
    if regular_bytes("/proc/1/comm", 128, owner=0).decode().strip() != "systemd":
        raise PackagingError("The real guest must boot systemd as PID 1")
    product = regular_bytes("/sys/class/dmi/id/product_name", 4096).decode().strip()
    vendor = regular_bytes("/sys/class/dmi/id/sys_vendor", 4096).decode().strip()
    identity = regular_bytes("/sys/class/dmi/id/product_uuid", 4096).decode().strip().lower()
    if product != "PHCI-Noble-TCG" or vendor != "PeripheralHub-CI" or identity != str(uuid.UUID(contract["nonce"])):
        raise PackagingError("Guest firmware identity does not match the concrete disposable QEMU job")
    mounts = regular_bytes("/proc/self/mountinfo", 1024 * 1024).decode().splitlines()
    matching = [line.split() for line in mounts if len(line.split()) > 8 and line.split()[4] == str(INPUT)]
    if len(matching) != 1 or "ro" not in matching[0][5].split(",") or matching[0][matching[0].index("-") + 1] != "iso9660":
        raise PackagingError("VM sources and bundle must arrive on one read-only ISO9660 input")
    on_disk = json_file(INPUT / "contract.json")
    if canonical(on_disk) != canonical(contract):
        raise PackagingError("Guest contract does not match its read-only input ISO")
    for name, info in contract["files"].items():
        path = INPUT / name
        if file_sha(path) != info["sha256"] or path.stat().st_size != info["size"]:
            raise PackagingError("Guest ISO input failed its expected checksum: " + name)
    import pwd
    user = pwd.getpwnam("phci")
    if user.pw_uid != 1000:
        raise PackagingError("The real guest test user must be phci UID 1000")
    release = parse_os_release(regular_bytes(Path("/etc/os-release").resolve(), 65536, owner=0).decode())
    if release.get("ID") != "ubuntu" or release.get("VERSION_ID") != "24.04":
        raise PackagingError("The guest is not a real Ubuntu 24.04 Noble system")
    return {"os_release": release, "pid1": "systemd", "architecture": platform.machine(),
            "kernel": run(["uname", "-r"]).stdout.strip(),
            "boot_id": regular_bytes("/proc/sys/kernel/random/boot_id", 256).decode().strip(),
            "dmi_uuid": identity, "dmi_product": product, "user": {"name": user.pw_name, "uid": user.pw_uid}}


def observe_checkpoint(name, transactions=None):
    commands = checkpoint_commands()
    observation = {"checkpoint": name, "commands": {}, "transactions": transactions or []}
    for label, argv in commands.items():
        result = run(argv, check=False, timeout=20)
        observation["commands"][label] = {"argv": argv, "returncode": result.returncode,
                                           "stdout": (result.stdout or "")[-8192:], "stderr": (result.stderr or "")[-4096:]}
    for label, path in (("autostart", Path("/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop")),
                        ("native_registry", Path("/var/lib/peripheral-hub/native-status.json"))):
        observation[label] = file_sha(path) if path.is_file() else None
    observation["payload_present"] = all(Path(path).is_file() for path in ("/usr/bin/peripheral-hub", "/usr/bin/peripheral-hub-remapping-agent", "/usr/bin/peripheral-hub-audio-worker", "/usr/lib/peripheral-hub/remapper-service"))
    observation["recovery_present"] = Path("/usr/share/applications/io.peripheralhub.Recovery.desktop").is_file()
    observation["native_failure_present"] = Path("/var/lib/peripheral-hub/native-failure.json").exists()
    return observation


def emit_report(contract, report):
    raw = canonical(report)
    if len(raw) > MAX_REPORT:
        raise PackagingError("Guest report exceeds its serial frame budget")
    header = "PHCI_VM_REPORT_V1 " + contract["nonce"] + " " + (contract["manifest_sha256"] or "none") + " " + contract["frame_sha256"]
    frame = "\n" + header + "\n" + base64.b64encode(raw).decode("ascii") + "\nPHCI_VM_END_V1 " + digest(raw) + "\n"
    with Path("/dev/ttyS0").open("wb", buffering=0) as serial:
        serial.write(frame.encode("ascii"))


def main():
    contract = json_file(INPUT / "contract.json")
    verify_contract(contract)
    report = {key: contract[key] for key in ("nonce", "target", "mode", "manifest_sha256", "frame_sha256")}
    report.update(schema=1, executor="qemu-tcg", native_smoke="not-run", ready=False, observations=[])
    try:
        report["guest"] = require_guest(contract)
        # These installs/services exist solely in this fresh VM, not in the host or SDK.
        run(["apt-get", "update"], capture=False, timeout=180)
        run(["apt-get", "install", "--yes", "--no-install-recommends", "dbus-user-session", "dbus", "systemd-sysv"], capture=False, timeout=300)
        run(["systemctl", "start", "user@1000.service"], capture=False)
        report["ready"] = True
        if contract["mode"] == "native":
            manifest, _ = verify_bundle(INPUT / "bundle", checked_sha(contract["manifest_sha256"]))
            require_software(manifest)
            from ci import smoke
            result = smoke.main(vm_contract=contract, observer=lambda name, transactions: report["observations"].append(observe_checkpoint(name, transactions)))
            report["native_smoke"] = result["native_smoke"]
            report["steps"] = result["steps"]
        else:
            report["observations"].append(observe_checkpoint("vm-readiness-only"))
        report["guest"]["versions"] = {name: run(["dpkg-query", "-W", "-f=${Version}\n", name], check=False).stdout.strip()
                                            for name in ("systemd", "dbus", "python3", "linux-image-generic")}
    except BaseException as error:
        report["error"] = str(error)[-8192:]
        report["native_smoke"] = "failed" if contract["mode"] == "native" else "not-run"
        report["ready"] = False
    emit_report(contract, report)
    return 0 if report["ready"] and report["native_smoke"] != "failed" else 1


if __name__ == "__main__":
    sys.exit(main())
