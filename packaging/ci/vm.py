#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Noble-only native gate in a disposable QEMU/TCG VM inside nonprivileged Docker."""

import argparse
import base64
import hashlib
import json
import os
import re
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import HERE, PackagingError, build_target, digest, json_file, regular_bytes, require_build_user, run, safe_relative, verify_bundle, write_json
from ci.matrix import inspect_image_id
from ci.gates import SOFTWARE_GATES, require_native_preconditions, require_unpromoted
from ci.native_report import validate_checkpoints

TARGET = "ubuntu-24.04"
SDK_IMAGE = "peripheral-hub-sdk:ubuntu-24.04-98bffd516864"
SDK_ID = "sha256:98bffd5168643b1f2d9d74e9e572983061536f84bc79ac090a8ba21eb98a4af7"
CLOUD_URL = "https://cloud-images.ubuntu.com/noble/current/"
CLOUD_FILE = "noble-server-cloudimg-amd64.img"
KEYRING = Path("/usr/share/keyrings/ubuntu-cloudimage-keyring.gpg")
FIRMWARE = Path("/usr/share/OVMF/OVMF_CODE_4M.fd")
VARIABLES = Path("/usr/share/OVMF/OVMF_VARS_4M.fd")
MAX_SERIAL = 16 * 1024 * 1024
MAX_REPORT = 1024 * 1024
NATIVE_STEPS = (
    "native-install", "native-hooks-autostart", "installed-headsetcontrol-api", "private-service-and-dbus",
    "normal-user-agent-and-recovery", "same-version-upgrade-hooks", "upgrade-preserves-autostart-config",
    "repair-missing-owned-file", "upgrade-preserves-disabled-service", "legacy-upgrade-preserves-disabled-service",
    "legacy-upgrade-preserves-stopped-service", "remove-reinstall-preserves-disabled-service",
    "remove-reinstall-preserves-stopped-service", "remove-reinstall-preserves-running-service",
    "uninstall-private-payload", "native-reinstall-idempotence",
)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii")


def checked_sha(value):
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{64}", value):
        raise PackagingError("An explicit complete lowercase SHA256 is required")
    return value


def file_sha(path):
    path = Path(path)
    if path.is_symlink() or not path.is_file() or path.stat().st_nlink != 1:
        raise PackagingError("A regular nonlinked VM input is required: " + str(path))
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def require_software(manifest):
    require_native_preconditions(manifest)
    target = build_target(manifest.get("target"), manifest.get("package", {}).get("format"))
    if target["id"] != TARGET or manifest["package"]["format"] != "deb":
        raise PackagingError("QEMU native smoke supports only a real Ubuntu 24.04 Noble DEB")


def checksum_for(sums):
    matches = []
    for line in sums.decode("ascii").splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1].lstrip("*") == CLOUD_FILE:
            matches.append(checked_sha(fields[0]))
    if len(matches) != 1:
        raise PackagingError("Signed Ubuntu sums must identify exactly one Noble amd64 cloud image")
    return matches[0]


def verify_cloud_signature(directory):
    directory = Path(directory)
    regular_bytes(KEYRING, 8 * 1024 * 1024, owner=0)
    sums = regular_bytes(directory / "SHA256SUMS", 1024 * 1024)
    regular_bytes(directory / "SHA256SUMS.gpg", 256 * 1024)
    result = run(["gpgv", "--status-fd", "1", "--keyring", str(KEYRING),
                  str(directory / "SHA256SUMS.gpg"), str(directory / "SHA256SUMS")], timeout=30)
    statuses = result.stdout.splitlines()
    valid = [line.split()[2] for line in statuses if line.startswith("[GNUPG:] VALIDSIG ")]
    if len(valid) != 1 or not re.fullmatch(r"[A-F0-9]{40,64}", valid[0]) or any(
            line.startswith("[GNUPG:] " + error) for line in statuses
            for error in ("BADSIG", "ERRSIG", "FAILURE", "NODATA", "EXPSIG", "EXPKEYSIG", "REVKEYSIG")):
        raise PackagingError("Ubuntu cloud image signed sums did not yield one trusted valid signature")
    return checksum_for(sums), valid[0]


def download(url, destination, maximum, deadline=1200):
    destination = Path(destination)
    started = time.monotonic()
    request = urllib.request.Request(url, headers={"User-Agent": "PeripheralHub-native-ci/1"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response, destination.open("xb") as output:
            resolved = urllib.parse.urlsplit(response.geturl())
            if resolved.scheme != "https" or resolved.hostname != "cloud-images.ubuntu.com":
                raise PackagingError("Ubuntu cloud download escaped its official HTTPS origin")
            count = 0
            while block := response.read(1024 * 1024):
                count += len(block)
                if count > maximum or time.monotonic() - started > deadline:
                    raise PackagingError("Ubuntu cloud download exceeded its byte/time budget")
                output.write(block)
    except BaseException:
        destination.unlink(missing_ok=True)
        raise


def prepare_cloud(cache):
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".signed-sums-", dir=cache) as temporary:
        metadata = Path(temporary)
        for name, maximum in (("SHA256SUMS", 1024 * 1024), ("SHA256SUMS.gpg", 256 * 1024)):
            download(CLOUD_URL + name, metadata / name, maximum)
        expected, signer = verify_cloud_signature(metadata)
        directory = cache / expected
        directory.mkdir(exist_ok=True)
        image = directory / CLOUD_FILE
        if image.exists():
            if file_sha(image) != expected:
                raise PackagingError("Cached Noble cloud image differs from Ubuntu's signed checksum")
        else:
            partial = directory / (".download-" + uuid.uuid4().hex)
            try:
                download(CLOUD_URL + CLOUD_FILE, partial, 2 * 1024 ** 3)
                if file_sha(partial) != expected:
                    raise PackagingError("Noble cloud image SHA256 does not match trusted signed sums")
                partial.replace(image)
            finally:
                partial.unlink(missing_ok=True)
        for name in ("SHA256SUMS", "SHA256SUMS.gpg"):
            shutil.copyfile(metadata / name, directory / name)
    verified, verified_signer = verify_cloud_signature(directory)
    if verified != expected or signer != verified_signer:
        raise PackagingError("Cached Ubuntu signature metadata changed")
    info = json.loads(run(["qemu-img", "info", "--output=json", str(image)], timeout=30).stdout)
    if info.get("format") != "qcow2" or info.get("backing-filename"):
        raise PackagingError("Official cloud image must be a standalone qcow2 image")
    evidence = {"url": CLOUD_URL + CLOUD_FILE, "sha256": expected, "signer": signer,
                "signed_sums_sha256": file_sha(directory / "SHA256SUMS"), "keyring": str(KEYRING)}
    write_json(cache / "verified.json", evidence)
    return evidence


def input_files(bundle=None):
    files = {}
    source = HERE
    for name in ("common.py", "build.py", "sources.py", "staging.py", "targets.json"):
        files["source/packaging/" + name] = source / name
    for name in ("smoke.py", "gates.py", "matrix.py", "vm.py", "vm_guest.py", "native_report.py"):
        files["source/packaging/ci/" + name] = source / "ci" / name
    if bundle is not None:
        manifest, _ = verify_bundle(bundle)
        for name in ("manifest.json", "manifest.sha256", *manifest["files"]):
            files["bundle/" + name] = Path(bundle).joinpath(*safe_relative(name).parts)
    return files


def make_contract(nonce, cloud, bundle=None, manifest_sha=None):
    uuid.UUID(nonce)
    if len(nonce) != 32 or nonce != nonce.lower():
        raise PackagingError("VM job nonce must be canonical UUID hex")
    if bundle is not None:
        manifest, _ = verify_bundle(bundle, checked_sha(manifest_sha))
        require_software(manifest)
        require_unpromoted(manifest)
    files = {name: {"sha256": file_sha(path), "size": path.stat().st_size}
             for name, path in input_files(bundle).items()}
    contract = {"schema": 1, "target": TARGET, "nonce": nonce, "mode": "native" if bundle else "readiness",
                "manifest_sha256": manifest_sha, "cloud": cloud, "files": files}
    if bundle is not None:
        contract["package"] = {"name": manifest["app"]["name"], "file": manifest["package"]["file"],
                               "version": manifest["package"]["version"],
                               "sha256": manifest["files"][manifest["package"]["file"]]["sha256"]}
    contract["frame_sha256"] = digest(canonical(contract))
    return contract


def verify_contract(contract):
    if contract.get("schema") != 1 or contract.get("target") != TARGET or contract.get("mode") not in ("native", "readiness"):
        raise PackagingError("Unsupported VM input contract/target")
    nonce = contract.get("nonce", "")
    if not re.fullmatch(r"[a-f0-9]{32}", nonce):
        raise PackagingError("Invalid VM job nonce")
    claimed = checked_sha(contract.get("frame_sha256"))
    if digest(canonical({key: value for key, value in contract.items() if key != "frame_sha256"})) != claimed:
        raise PackagingError("VM input contract frame checksum mismatch")
    if contract["mode"] == "native":
        checked_sha(contract.get("manifest_sha256"))
        package = contract.get("package")
        if not isinstance(package, dict) or set(package) != {"name", "file", "version", "sha256"} or package["name"] != "peripheral-hub" or not isinstance(package["version"], str) or not re.fullmatch(r"[0-9][A-Za-z0-9.+:~_-]{0,100}", package["version"]):
            raise PackagingError("Native VM contract must bind the exact expected package/version")
        safe_relative(package["file"])
        checked_sha(package["sha256"])
    checked_sha(contract.get("cloud", {}).get("sha256"))
    if contract["cloud"].get("url") != CLOUD_URL + CLOUD_FILE:
        raise PackagingError("VM cloud provenance is not the official Noble image")
    if not isinstance(contract.get("files"), dict) or not 1 <= len(contract["files"]) <= 12000:
        raise PackagingError("Invalid VM input file inventory")
    for name, info in contract["files"].items():
        safe_relative(name)
        if not name.startswith(("source/packaging/", "bundle/")) or not isinstance(info, dict) or type(info.get("size")) is not int or not 0 <= info["size"] <= 2 ** 31:
            raise PackagingError("Unsafe VM input file inventory")
        checked_sha(info.get("sha256"))


def qemu_command(work, nonce):
    work = Path(work).resolve()
    if any(character in str(work) for character in (",", "\n", "\r")):
        raise PackagingError("VM work path cannot inject QEMU drive options")
    if not re.fullmatch(r"[a-f0-9]{32}", nonce):
        raise PackagingError("Invalid VM identity")
    return ["qemu-system-x86_64", "-nodefaults", "-no-user-config", "-accel", "tcg,thread=multi",
            "-machine", "q35", "-cpu", "max", "-m", "4096", "-smp", "2", "-display", "none",
            "-monitor", "none", "-serial", "stdio", "-no-reboot",
            "-smbios", "type=1,manufacturer=PeripheralHub-CI,product=PHCI-Noble-TCG,uuid=" + str(uuid.UUID(nonce)),
            "-drive", f"if=pflash,format=raw,readonly=on,file={FIRMWARE}",
            "-drive", f"if=pflash,format=raw,file={work / 'vars.fd'}",
            "-drive", f"if=none,id=os,format=qcow2,file={work / 'overlay.qcow2'}", "-device", "virtio-blk-pci,drive=os,bootindex=0",
            "-drive", f"if=none,id=seed,format=raw,readonly=on,file={work / 'seed.iso'}", "-device", "virtio-blk-pci,drive=seed",
            "-drive", f"if=none,id=input,format=raw,readonly=on,file={work / 'input.iso'}", "-device", "virtio-blk-pci,drive=input",
            "-netdev", "user,id=net0,ipv6=off", "-device", "virtio-net-pci,netdev=net0"]


def validate_qemu_args(command, work, nonce):
    if command != qemu_command(work, nonce):
        raise PackagingError("QEMU arguments differ from the closed TCG/no-host-device/no-forwarding policy")
    for name in ("overlay.qcow2", "vars.fd", "seed.iso", "input.iso"):
        path = Path(work) / name
        if path.is_symlink() or not path.is_file() or path.stat().st_nlink != 1:
            raise PackagingError("Unsafe QEMU drive input: " + name)


def monitor_qemu(command, serial, deadline):
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, start_new_session=True)
    started = time.monotonic()
    count = 0
    try:
        with selectors.DefaultSelector() as selector, Path(serial).open("xb") as output:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() - started > deadline:
                    raise PackagingError("Disposable VM exceeded its deadline")
                for key, _ in selector.select(timeout=1):
                    block = os.read(key.fileobj.fileno(), 65536)
                    if not block:
                        selector.unregister(key.fileobj)
                        continue
                    count += len(block)
                    if count > MAX_SERIAL:
                        raise PackagingError("VM serial evidence exceeded its byte budget")
                    output.write(block)
                    output.flush()
            status = process.wait(timeout=max(1, deadline - (time.monotonic() - started)))
            if status:
                raise PackagingError("QEMU exited unsuccessfully: " + str(status))
    finally:
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=10)
        process.stdout.close()


def decode_report(serial, expected):
    verify_contract(expected)
    text = regular_bytes(serial, MAX_SERIAL).decode("utf-8", errors="replace")
    header = "PHCI_VM_REPORT_V1 " + expected["nonce"] + " " + (expected["manifest_sha256"] or "none") + " " + expected["frame_sha256"]
    pattern = r"(?m)^" + re.escape(header) + r"\r?\n([A-Za-z0-9+/=]+)\r?\nPHCI_VM_END_V1 ([a-f0-9]{64})\r?$"
    matches = re.findall(pattern, text)
    if len(matches) != 1 or text.count("PHCI_VM_REPORT_V1 ") != 1 or text.count("PHCI_VM_END_V1 ") != 1:
        raise PackagingError("VM report is missing, duplicated or does not match the expected input frame")
    raw = base64.b64decode(matches[0][0], validate=True)
    if len(raw) > MAX_REPORT or digest(raw) != matches[0][1]:
        raise PackagingError("VM report frame was altered or oversized")
    report = json.loads(raw)
    for key in ("nonce", "target", "mode", "manifest_sha256", "frame_sha256"):
        if report.get(key) != expected[key]:
            raise PackagingError("VM report belongs to another input/job: " + key)
    guest = report.get("guest", {})
    release = guest.get("os_release", {})
    if report.get("schema") != 1 or report.get("executor") != "qemu-tcg" or release.get("ID") != "ubuntu" or release.get("VERSION_ID") != "24.04" or guest.get("pid1") != "systemd" or guest.get("architecture") != "x86_64" or guest.get("dmi_uuid", "").lower() != str(uuid.UUID(expected["nonce"])) or guest.get("user", {}).get("uid") != 1000 or guest.get("user", {}).get("name") != "phci":
        raise PackagingError("VM report lacks actual Noble/systemd/identity/user provenance: " + str(report.get("error", "unknown guest identity")))
    if not isinstance(guest.get("kernel"), str) or not guest["kernel"] or not re.fullmatch(r"[a-f0-9-]{32,36}", guest.get("boot_id", "")) or any(not guest.get("versions", {}).get(name) for name in ("systemd", "dbus", "python3")):
        raise PackagingError("VM report lacks actual guest kernel/boot/version evidence")
    if expected["mode"] == "readiness":
        if report.get("native_smoke") != "not-run" or report.get("ready") is not True:
            raise PackagingError("Readiness boot cannot certify native package smoke")
    else:
        validate_checkpoints(report, expected, NATIVE_STEPS)
    return report


def worker_boot(contract, image, bundle, job):
    verify_contract(contract)
    image = Path(image)
    expected, signer = verify_cloud_signature(image.parent)
    if expected != contract["cloud"]["sha256"] or signer != contract["cloud"]["signer"] or file_sha(image) != expected:
        raise PackagingError("VM boot image differs from trusted signed cloud provenance")
    job = Path(job)
    with tempfile.TemporaryDirectory(prefix=".vm-", dir=job) as temporary:
        work = Path(temporary)
        payload = work / "input"
        payload.mkdir()
        files = input_files(bundle)
        if set(files) != set(contract["files"]):
            raise PackagingError("VM source/bundle inventory changed before launch")
        for name, original in files.items():
            destination = payload.joinpath(*safe_relative(name).parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, destination)
            if file_sha(destination) != contract["files"][name]["sha256"] or destination.stat().st_size != contract["files"][name]["size"]:
                raise PackagingError("VM input changed before read-only ISO transfer: " + name)
        write_json(payload / "contract.json", contract)
        run(["genisoimage", "-quiet", "-R", "-J", "-V", "PHCI_INPUT", "-o", str(work / "input.iso"), str(payload)], timeout=180)
        user_data = """#cloud-config
users:
  - name: phci
    uid: 1000
    shell: /bin/bash
    lock_passwd: true
ssh_pwauth: false
disable_root: true
ssh_deletekeys: true
bootcmd:
  - [systemctl, mask, ssh.service, ssh.socket]
growpart:
  mode: auto
  devices: ['/']
resize_rootfs: true
runcmd:
  - [mkdir, -p, /mnt/phci]
  - [mount, -t, iso9660, -o, ro, /dev/disk/by-label/PHCI_INPUT, /mnt/phci]
  - [python3, -B, /mnt/phci/source/packaging/ci/vm_guest.py]
  - [poweroff]
"""
        (work / "user-data").write_text(user_data, encoding="ascii")
        write_json(work / "meta-data", {"instance-id": "phci-" + contract["nonce"], "local-hostname": "phci-noble"})
        run(["cloud-localds", str(work / "seed.iso"), str(work / "user-data"), str(work / "meta-data")], timeout=30)
        shutil.copyfile(VARIABLES, work / "vars.fd")
        run(["qemu-img", "create", "-f", "qcow2", "-F", "qcow2", "-b", str(image.resolve()), str(work / "overlay.qcow2")], timeout=30)
        run(["qemu-img", "resize", str(work / "overlay.qcow2"), "20G"], timeout=30)
        command = qemu_command(work, contract["nonce"])
        validate_qemu_args(command, work, contract["nonce"])
        monitor_qemu(command, job / "serial.log", contract.get("deadline", 2400))
    report = decode_report(job / "serial.log", contract)
    write_json(job / "report.json", report)
    return report


def docker_command(runtime, image, cache, job=None, bundle=None, name=None):
    command = [*runtime, "run", "--read-only", "--user", "1000:1000", "--cap-drop", "ALL",
               "--security-opt", "no-new-privileges", "--network", "bridge", "--ipc", "private", "--cgroupns", "private",
               "--pids-limit", "512", "--memory", "6g", "--tmpfs", "/tmp:rw,nosuid,nodev,mode=1777",
               "--volume", f"{HERE.parent.resolve()}:/workspace:ro", "--volume", f"{cache}:/images:{'ro' if job else 'rw'}"]
    if name:
        command.extend(["--name", name])
    if job:
        command.extend(["--volume", f"{job}:/job:rw"])
    if bundle:
        command.extend(["--volume", f"{Path(bundle).resolve()}:/bundle:ro"])
    command.extend([image, "--target", TARGET])
    return command


def run_container(runtime, image, cache, job=None, bundle=None, timeout=1500, download_only=False):
    name = "peripheral-hub-vm-" + uuid.uuid4().hex
    command = docker_command(runtime, image, cache, job, bundle, name)
    if download_only:
        command.append("--download-only")
    try:
        run(command, capture=False, timeout=timeout)
    finally:
        # The name is generated by this invocation, never taken from user input.
        run([*runtime, "rm", "--force", name], check=False, capture=False, timeout=30)


def prepare_runtime(runtime):
    if inspect_image_id(runtime, SDK_IMAGE) != SDK_ID:
        raise PackagingError("The immutable SDK98 base changed; refusing VM addon build")
    image = "peripheral-hub-vm:ubuntu-24.04"
    run([*runtime, "build", "--platform", "linux/amd64", "--file", str(HERE / "ci/VM.Dockerfile"),
         "--build-arg", "SDK_IMAGE=" + SDK_IMAGE, "--tag", image, str(HERE.parent)], capture=False, timeout=1200)
    identifier = inspect_image_id(runtime, image)
    tag = "peripheral-hub-vm:ubuntu-24.04-" + identifier.removeprefix("sha256:")[:12]
    run([*runtime, "tag", image, tag])
    return tag, identifier


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default=TARGET)
    parser.add_argument("--host-engine", action="store_true")
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--boot-check", action="store_true")
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--manifest-sha256")
    parser.add_argument("--cache", type=Path, default=HERE / ".cache/noble-vm")
    parser.add_argument("--deadline", type=int, default=2400)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--download-only", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    require_build_user()
    if args.target != TARGET or not 120 <= args.deadline <= 3600:
        raise PackagingError("VM smoke supports only Ubuntu24.04 and bounded 120..3600s deadlines")
    if args.worker:
        if os.environ.get("PH_CI_VM_BUILDER") != "1" or not any(Path(name).is_file() for name in ("/.dockerenv", "/run/.containerenv")):
            raise PackagingError("VM worker requires the dedicated non-root OCI VM addon")
        if args.download_only:
            print(json.dumps(prepare_cloud(Path("/images")), sort_keys=True))
        else:
            contract = json_file(Path("/job/contract.json"))
            bundle = Path("/bundle") if contract["mode"] == "native" else None
            cloud = Path("/images") / contract["cloud"]["sha256"] / CLOUD_FILE
            report = worker_boot(contract, cloud, bundle, Path("/job"))
            print(json.dumps({"mode": report["mode"], "native_smoke": report["native_smoke"], "nonce": report["nonce"]}))
        return
    if not args.prepare_only:
        if args.bundle is None:
            raise PackagingError("A quarantine bundle and its expected manifest SHA256 are required")
        manifest, _ = verify_bundle(args.bundle, checked_sha(args.manifest_sha256))
        require_software(manifest)
        require_unpromoted(manifest)
    cache_base = HERE / ".cache/noble-vm"
    if any(path.is_symlink() for path in (cache_base, cache_base.parent)):
        raise PackagingError("VM cache cannot follow a project-external symlink")
    cache = args.cache.resolve()
    if not cache.is_relative_to(cache_base.resolve()) or not cache.is_relative_to(HERE.parent.resolve()):
        raise PackagingError("VM cache must stay inside packaging/.cache/noble-vm")
    cache.mkdir(parents=True, exist_ok=True)
    runtime = (["flatpak-spawn", "--host"] if args.host_engine else []) + ["docker"]
    image, image_id = prepare_runtime(runtime)
    run_container(runtime, image, cache, download_only=True)
    cloud = json_file(cache / "verified.json")
    if args.prepare_only and not args.boot_check:
        print(json.dumps({"vm_sdk": "ready", "image": image, "image_id": image_id, "cloud": cloud,
                          "native_smoke": "not-run"}, indent=2))
        return
    nonce = uuid.uuid4().hex
    jobs = cache / "jobs"
    jobs.mkdir(exist_ok=True)
    job = jobs / nonce
    job.mkdir(mode=0o700)
    bundle = None if args.prepare_only else args.bundle
    contract = make_contract(nonce, cloud, bundle, None if args.prepare_only else args.manifest_sha256)
    contract["deadline"] = args.deadline
    contract["frame_sha256"] = digest(canonical({key: value for key, value in contract.items() if key != "frame_sha256"}))
    write_json(job / "contract.json", contract)
    try:
        run_container(runtime, image, cache, job, bundle, timeout=args.deadline + 300)
        report = decode_report(job / "serial.log", contract)
        if canonical(report) != canonical(json_file(job / "report.json")):
            raise PackagingError("Extracted VM report differs from serial evidence")
        if bundle:
            evidence = {"executor": "qemu-tcg", "scope": "disposable-noble-systemd-package-lifecycle",
                        "vm_image_id": image_id, "cloud": cloud, "report_sha256": digest(canonical(report)),
                        "input_manifest_sha256": args.manifest_sha256, "frame_sha256": contract["frame_sha256"],
                        "nonce": nonce, "guest": report["guest"], "observations": report["observations"]}
            from build import update_validation
            update_validation(bundle, args.manifest_sha256, {"native_smoke": "passed", "native_smoke_steps": report["steps"],
                "native_smoke_evidence": evidence, "linux_hardware": "not-run", "graphical_login_reboot": "not-run",
                "audio_hardware": "not-run", "dkms_host_kernel": "not-run",
                "native_smoke_limitations": "Real disposable QEMU/TCG Noble package/systemd/user-bus lifecycle only; no host HID, audio, DKMS, graphical login or reboot certification"})
        print(json.dumps({"mode": report["mode"], "native_smoke": report["native_smoke"], "report": str(job / "report.json"), "vm_image": image}, indent=2))
    except BaseException:
        # Do not promote, delete foreign resources, or silently retry a failed native gate.
        write_json(job / "failed.json", {"native_smoke": "not-run", "nonce": nonce, "manifest_sha256": args.manifest_sha256})
        raise


if __name__ == "__main__":
    try:
        main()
    except (PackagingError, OSError, ValueError, subprocess.SubprocessError) as error:
        print("VM gate failed: " + str(error), file=sys.stderr)
        sys.exit(1)
