#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Shared, dependency-free build and installer validation. No host mutations."""

import base64
import copy
import hashlib
import json
import os
import platform
import re
import shlex
import stat
import subprocess
import sys
import tempfile
import urllib.request
from datetime import date
from pathlib import Path, PurePosixPath

HERE = Path(__file__).resolve().parent
APP = "peripheral-hub"
AGENT = "peripheral-hub-remapping-agent.service"
ENGINE = "peripheral-hub-input-remapper.service"
STATE = Path("/var/lib/peripheral-hub/installer.json")
NATIVE_STATE = STATE.with_name("native-status.json")
NATIVE_FAILURE = STATE.with_name("native-failure.json")
HEADSET = Path("/usr/lib/peripheral-hub/headsetcontrol")
HEADSET_REPLUG_NOTICE = "headsetcontrol: reconnect existing headsets if new uaccess rules have not taken effect"
MAX_JSON = 8 * 1024 * 1024
SAFE_PACKAGE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9+_.:@() /<>=|~-]*$")


class PackagingError(RuntimeError):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest()


def regular_bytes(path, maximum=MAX_JSON, owner=None):
    path = Path(path)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0)
    if path.is_symlink():
        raise PackagingError(f"Symlink forbidden: {path}")
    with os.fdopen(os.open(path, flags), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > maximum:
            raise PackagingError(f"Not a bounded, single-link regular file: {path}")
        if owner is not None and info.st_uid != owner:
            raise PackagingError(f"Unexpected file owner: {path}")
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise PackagingError(f"File too large: {path}")
    return data


def json_file(path):
    try:
        value = json.loads(regular_bytes(path))
    except (ValueError, OSError) as error:
        raise PackagingError(f"Invalid JSON file {path}: {error}") from error
    if not isinstance(value, dict):
        raise PackagingError(f"Expected JSON object: {path}")
    return value


def native_failure(path=NATIVE_FAILURE, owner=0):
    path = Path(path)
    try:
        directory = path.parent.lstat()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise PackagingError("Cannot verify native state directory") from error
    if not stat.S_ISDIR(directory.st_mode) or directory.st_uid != owner or directory.st_mode & 0o022:
        raise PackagingError("Unsafe native state directory; configuration outcome unknown")
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise PackagingError("Cannot verify native configuration checkpoint") from error
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != owner or info.st_mode & 0o022 or info.st_size > 32768:
        raise PackagingError("Unsafe native configuration checkpoint")
    try:
        record = json.loads(regular_bytes(path, 32768, owner=owner))
    except (OSError, ValueError) as error:
        raise PackagingError("Invalid native configuration checkpoint") from error
    if not isinstance(record, dict) or set(record) != {"schema", "app", "engine", "format", "status", "error"} or type(record.get("schema")) is not int or record["schema"] != 1:
        raise PackagingError("Unknown native configuration checkpoint schema; preserved")
    if record["app"] != APP or record["engine"] != ENGINE or record["format"] not in ("deb", "rpm", "arch") or record["status"] not in ("configuring", "failed") or not isinstance(record["error"], str) or len(record["error"]) > 4000:
        raise PackagingError("Invalid native configuration checkpoint fields; preserved")
    return record


def write_json(path, data, mode=0o644):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix="." + path.name + "-", dir=path.parent)
    try:
        os.chmod(temporary, mode)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as output:
            json.dump(data, output, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def run(argv, check=True, cwd=None, env=None, timeout=120, capture=True):
    if not isinstance(argv, list) or not argv or not all(isinstance(arg, str) for arg in argv):
        raise PackagingError("Commands must be argument arrays, never a shell string")
    try:
        result = subprocess.run(
            argv, cwd=cwd, env=env if env is not None else dict(os.environ, LC_ALL="C", LANG="C"), timeout=timeout, check=False,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.PIPE if capture else None, text=True,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PackagingError(f"Cannot run {argv[0]}: {error}") from error
    if check and result.returncode:
        raise PackagingError(f"{shlex.join(argv)} failed ({result.returncode}): {(result.stderr or '')[-4000:]}")
    return result


def require_build_user():
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise PackagingError("Native builds require a Linux x86_64 build environment")
    if os.geteuid() == 0:
        raise PackagingError("Building/fetching as root is forbidden")
    if sys.version_info < (3, 10):
        raise PackagingError("Python >= 3.10 is required")


def load_targets(path=None):
    catalog = json_file(path or HERE / "targets.json")
    if catalog.get("schema") != 1:
        raise PackagingError("Unsupported target catalog schema")
    raw = {target["id"]: target for target in catalog["targets"]}

    def resolve(key, seen=()):
        if key in seen or key not in raw:
            raise PackagingError("Invalid target inheritance")
        child = copy.deepcopy(raw[key])
        target = resolve(child["inherit"], seen + (key,)) if "inherit" in child else {}
        target.update(child)
        for old, new in child.get("runtime_replace", {}).items():
            target["runtime"] = [new if item == old else item for item in target["runtime"]]
        target.pop("inherit", None)
        target.pop("runtime_replace", None)
        # Inheriting a rolling target must not retain another distro's release policy.
        if child.get("rolling"):
            target.pop("codename", None)
        return target

    catalog["targets"] = [resolve(key) for key in raw]
    return catalog


def parse_os_release(text):
    result = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise PackagingError("Malformed os-release")
        key, value = line.split("=", 1)
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or key in result:
            raise PackagingError("Invalid or duplicate os-release key")
        try:
            parsed = shlex.split(value)
        except ValueError as error:
            raise PackagingError("Invalid quoting in os-release") from error
        if len(parsed) > 1:
            raise PackagingError("Invalid os-release value")
        result[key] = parsed[0] if parsed else ""
    if not result.get("ID"):
        raise PackagingError("os-release ID is required; ID_LIKE is not a support claim")
    return result


def target_for(release, catalog=None, today=None, allow_compatible=True):
    if not isinstance(release, dict):
        raise PackagingError("Expected os-release data")
    catalog = catalog or load_targets()
    for target in catalog["targets"]:
        native = target["os_id"] == release.get("ID") and (target.get("rolling") or release.get("VERSION_ID") in target["version_ids"])
        compatible = allow_compatible and any(
            rule["target"] == target["id"] and all(release.get(key) == value for key, value in rule["os_release"].items())
            for rule in catalog.get("host_compatibility", [])
        )
        if not native and not compatible:
            continue
        if "support_until" in target and (today or date.today()) >= date.fromisoformat(target["support_until"]):
            raise PackagingError(f"Target {target['id']} has expired; rebuild against a maintained Fedora release")
        if target.get("codename") and release.get("VERSION_CODENAME", target["codename"]) != target["codename"]:
            raise PackagingError("Distribution codename/version mismatch")
        return target
    raise PackagingError(f"Unsupported target: {release.get('ID')} {release.get('VERSION_ID', '')}")


def build_target(provenance, package_format):
    if not isinstance(provenance, dict) or not isinstance(provenance.get("os_release"), dict) or not re.fullmatch(r"sha256:[a-f0-9]{64}", str(provenance.get("image_digest", ""))):
        raise PackagingError("Bundle lacks the actual build image digest and os-release provenance")
    # Compatibility is an installation policy, never evidence of a native build.
    target = target_for(provenance["os_release"], allow_compatible=False)
    if provenance.get("id") != target["id"] or package_format != target["format"]:
        raise PackagingError("Build provenance differs from the native target/format")
    if provenance.get("image") is not None and provenance["image"] != target["image"]:
        raise PackagingError("Build image differs from the declared distribution/version")
    return target


def safe_relative(name):
    if not isinstance(name, str) or not name or "\\" in name or ":" in name or any(ord(char) < 32 for char in name):
        raise PackagingError("Invalid artifact path")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in ("", ".", "..") for part in name.split("/")):
        raise PackagingError("Artifact path escapes bundle")
    return path


def bundle_file(root, name):
    relative = safe_relative(name)
    root = Path(root).resolve()
    candidate = root.joinpath(*relative.parts)
    for parent in [candidate, *candidate.parents]:
        if parent == root:
            break
        if parent.is_symlink():
            raise PackagingError(f"Symlink in bundle path: {name}")
    if not candidate.resolve().is_relative_to(root):
        raise PackagingError("Artifact outside bundle")
    return candidate


def verify_bundle(root, expected=None):
    root = Path(root).resolve()
    raw = regular_bytes(root / "manifest.json")
    checksum = regular_bytes(root / "manifest.sha256", 256).decode("ascii").split()
    if not checksum or not re.fullmatch(r"[a-f0-9]{64}", checksum[0]) or checksum[0] != digest(raw):
        raise PackagingError("Manifest SHA256 mismatch")
    if expected is not None and digest(raw) != expected:
        raise PackagingError("Manifest changed between user preflight and administrative phase")
    manifest = json.loads(raw)
    if manifest.get("schema") != 1 or manifest.get("app", {}).get("name") != APP:
        raise PackagingError("Unsupported bundle manifest")
    if manifest["app"].get("version") != "0.1.0":
        raise PackagingError("Bundle version does not match this installer (0.1.0)")
    if manifest.get("architecture") != "x86_64" or manifest.get("init") != "systemd":
        raise PackagingError("Invalid bundle architecture/init")
    files = manifest.get("files")
    if not isinstance(files, dict) or not files or len(files) > 10000:
        raise PackagingError("Missing or oversized bundle file inventory")
    for name, entry in files.items():
        path = bundle_file(root, name)
        if not isinstance(entry, dict) or not isinstance(entry.get("size"), int) or not 0 <= entry["size"] <= 2 ** 31:
            raise PackagingError(f"Invalid file inventory: {name}")
        data = regular_bytes(path, entry["size"] + 1)
        if len(data) != entry["size"] or digest(data) != entry.get("sha256"):
            raise PackagingError(f"Artifact checksum mismatch: {name}")
    package = manifest.get("package", {})
    if package.get("file") not in files or package.get("format") not in ("deb", "rpm", "arch"):
        raise PackagingError("Native package missing from verified inventory")
    build_target(manifest.get("target"), package["format"])
    for required in ("install.py", "common.py", "system.py", "targets.json"):
        if required not in files:
            raise PackagingError(f"Installer component missing: {required}")
    if not manifest.get("sources") or not all(item["file"] in files for item in manifest["sources"]):
        raise PackagingError("Corresponding GPL source artifacts missing")
    return manifest, digest(raw)


def package_name(requirement):
    if not isinstance(requirement, str) or not SAFE_PACKAGE.fullmatch(requirement):
        raise PackagingError(f"Unsafe package requirement: {requirement!r}")
    return re.split(r"\s|[<>=]", requirement, maxsplit=1)[0]


def repository_version(manager, package):
    name = package_name(package)
    zypper_capability = name == "python3" or "(" in name
    commands = {
        "apt": ["apt-cache", "policy", name],
        "pacman": ["pacman", "-Si", "--", name],
        "dnf": ["dnf", "repoquery", "--available", "--latest-limit=1", "--qf", "%{version}-%{release}", name],
        "zypper": ["zypper", "--xmlout", "--disable-system-resolvables", "search", "--details", "--match-exact", "--type", "package"],
    }
    if manager == "zypper":
        commands[manager].extend(["--provides", "--", package] if zypper_capability else ["--", name])
    response = run(commands[manager], check=False)
    if response.returncode:
        return None
    text = response.stdout
    if manager == "apt":
        match = re.search(r"Candidate:\s+(\S+)", text)
        return match[1] if match and match[1] != "(none)" else None
    if manager == "pacman":
        match = re.search(r"^Version\s*:\s*(\S+)", text, re.M)
        return match[1] if match else None
    if manager == "zypper":
        import xml.etree.ElementTree as ET
        try:
            solvables = ET.fromstring(text).findall(".//solvable")
        except ET.ParseError:
            return None
        versions = [item.get("edition") for item in solvables
                    if (item.get("name") == name or zypper_capability)
                    and item.get("repository") not in (None, "", "@System", "(System Packages)")
                    and item.get("edition")]
        return versions[-1] if versions else None
    versions = [line.strip() for line in text.splitlines() if re.match(r"^\d", line)]
    return versions[-1] if versions else None


def resolve_packages(target, requirements, query=repository_version, python_version=None):
    resolved = []
    missing = []
    for requirement in dict.fromkeys(requirements):
        name = package_name(requirement)
        candidates = [name]
        if target["manager"] == "zypper" and name.startswith("python3-"):
            version = python_version or sys.version_info[:2]
            candidates.append(f"python{version[0]}{version[1]}-{name[8:]}")
        for candidate in candidates:
            # Check the version of this virtual capability, not the provider RPM's edition.
            query_name = requirement if target["manager"] == "zypper" and name == "python3" else candidate
            version = query(target["manager"], query_name)
            if version:
                exact = requirement[len(name):]
                if exact.startswith("=") and not exact.startswith("==") and version != exact[1:]:
                    continue
                resolved.append({"name": candidate, "version": version, "requirement": requirement})
                break
        else:
            missing.append(name)
    return resolved, missing


def version_at_least(found, minimum):
    # Only compare an upstream numeric release; epoch/packaging release are not ABI evidence.
    found = found.split(":")[-1]
    a = tuple(int(part) for part in re.match(r"\d+(?:\.\d+)*", found)[0].split(".")) if re.match(r"\d", found) else ()
    b = tuple(int(part) for part in minimum.split("."))
    return bool(a) and (a + (0,) * max(0, len(b) - len(a))) >= b


def headset_contract(output):
    if len(output) > 256 * 1024:
        return False, "JSON exceeds the Rust provider limit"
    def reject_constant(value):
        raise ValueError("Non-finite JSON number")
    try:
        value = json.loads(output.decode("utf-8"), parse_constant=reject_constant)
    except (ValueError, UnicodeError):
        return False, "CLI did not emit JSON"
    if not isinstance(value, dict) or value.get("name") != "HeadsetControl" or value.get("api_version") != "1.6":
        return False, "Verified HeadsetControl JSON API 1.6 is required"
    devices = value.get("devices")
    if not isinstance(devices, list) or len(devices) > 256:
        return False, "Invalid devices inventory"
    if type(value.get("device_count")) is not int or value["device_count"] != len(devices):
        return False, "Inconsistent device_count"
    for device in devices:
        if not isinstance(device, dict):
            return False, "Invalid device record"
        for key in ("id_vendor", "id_product"):
            if not isinstance(device.get(key), str) or any(character in device[key] for character in "\x1c\x1d\x1e\x1f") or not re.fullmatch(r"(?:0[xX])?[0-9a-fA-F]{1,4}", device[key].strip()):
                return False, f"Invalid {key}"
        if not isinstance(device.get("device"), str) or device.get("status") not in ("success", "partial", "failure", "unknown"):
            return False, "Invalid device name/status"
        for key in ("capabilities",):
            caps = device.get(key)
            if not isinstance(caps, list) or len(caps) > 128 or not all(isinstance(cap, str) for cap in caps):
                return False, f"Missing/invalid {key} in the verified JSON schema"
        if "errors" in device and (not isinstance(device["errors"], dict) or not all(isinstance(error, str) for error in device["errors"].values())):
            return False, "Invalid errors record"
        if "battery" in device:
            battery = device["battery"]
            if (not isinstance(battery, dict)
                    or battery.get("status") not in ("BATTERY_AVAILABLE", "BATTERY_CHARGING", "BATTERY_UNAVAILABLE", "BATTERY_ERROR", "BATTERY_TIMEOUT", "UNKNOWN")
                    or type(battery.get("level")) is not int or not -1 <= battery["level"] <= 100
                    or battery["status"] == "BATTERY_AVAILABLE" and battery["level"] < 0):
                return False, "Invalid battery record"
    return True, f"CLI schema {value['api_version']} verified; hardware support is unchanged"


def probe_headset(executable):
    report = {"configured": False, "user_access": "unknown", "probe_uid": os.geteuid()}
    if not executable:
        return dict(report, reason="CLI is not installed")
    report["path"] = str(executable)
    try:
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            def bound_files():
                import resource
                resource.setrlimit(resource.RLIMIT_FSIZE, (256 * 1024 + 1, 256 * 1024 + 1))
            result = subprocess.run(
                [str(executable), "-b", "-o", "json"], stdin=subprocess.DEVNULL,
                stdout=stdout, stderr=stderr, timeout=8, check=False,
                preexec_fn=bound_files if sys.platform == "linux" else None,
            )
            stdout.seek(0)
            stderr.seek(0)
            output = stdout.read(256 * 1024 + 1)
            errors = stderr.read(64 * 1024 + 1)
    except (OSError, subprocess.TimeoutExpired) as error:
        return dict(report, reason=str(error))
    good, reason = headset_contract(output)
    # Upstream 4.2.0 outputs a valid empty inventory and exits 1 when no headset
    # is attached. That verifies the software contract, not a battery reading.
    devices = json.loads(output)["devices"] if good else []
    if good:
        report["device_count"] = len(devices)
    no_devices = good and not devices
    if (result.returncode != 0 and not (result.returncode == 1 and no_devices)) or errors.strip():
        good, reason = False, "CLI probe failed or reported stderr; no confirmed battery samples"
    elif good and any(
        device.get("errors") or device.get("status") in ("failure", "partial")
        or isinstance(device.get("battery"), dict) and device["battery"].get("status") in ("BATTERY_ERROR", "BATTERY_TIMEOUT")
        for device in devices
    ):
        good, reason = False, "CLI reported a headset read error; check user access and device state"
    # Enumeration (especially an empty inventory) and root reads do not prove
    # desktop-user permissions. Require actual successful user battery samples.
    if good and report["probe_uid"] > 0 and devices and all(
        device.get("status") == "success" and int(device["id_vendor"].strip(), 16) in (0x046d, 0x1532)
        and "CAP_BATTERY_STATUS" in device["capabilities"] and isinstance(device.get("battery"), dict)
        and device["battery"].get("status") in ("BATTERY_AVAILABLE", "BATTERY_CHARGING")
        and type(device["battery"].get("level")) is int and 0 <= device["battery"]["level"] <= 100
        for device in devices
    ):
        report["user_access"] = "verified"
    if good and report["user_access"] == "unknown":
        reason += "; desktop-user access remains unknown until a successful user battery read"
    return dict(report, configured=good, reason=reason)


def https_bytes(url, maximum=32 * 1024 * 1024):
    if not url.startswith("https://"):
        raise PackagingError("Only HTTPS downloads are allowed")
    request = urllib.request.Request(url, headers={"User-Agent": "peripheral-hub-packaging/0.1.0"})
    with urllib.request.urlopen(request, timeout=90) as response:
        if not response.url.startswith("https://"):
            raise PackagingError("Download redirected to an insecure URL")
        data = response.read(maximum + 1)
    if len(data) > maximum:
        raise PackagingError("Download exceeds size limit")
    return data


def pgp_fingerprint(armored):
    """Validate one complete v4 public certificate before importing its bytes."""
    if len(armored) > 128 * 1024:
        raise PackagingError("Repository public key exceeds size limit")
    try:
        text = armored.decode("ascii").replace("\r\n", "\n")
    except UnicodeDecodeError as error:
        raise PackagingError("Expected an ASCII-armored repository public key") from error
    begin, end = "-----BEGIN PGP PUBLIC KEY BLOCK-----", "-----END PGP PUBLIC KEY BLOCK-----"
    match = re.fullmatch(r"[ \t\n]*" + begin + r"[ \t]*\n(.*?)\n" + end + r"[ \t\n]*", text, re.S)
    if text.count(begin) != 1 or text.count(end) != 1 or not match:
        raise PackagingError("Expected exactly one armored repository public key, without trailing data")
    lines = [line.strip(" \t") for line in match[1].split("\n")]
    while lines and re.fullmatch(r"[A-Za-z][A-Za-z0-9 -]*:[\t\x20-\x7e]*", lines[0]):
        lines.pop(0)
    if not lines or lines.pop(0):
        raise PackagingError("Invalid armored public key header separator")
    lines = [line for line in lines if line]
    checksum = None
    if lines and lines[-1].startswith("="):
        if not re.fullmatch(r"=[A-Za-z0-9+/]{4}", lines[-1]):
            raise PackagingError("Invalid armored public key checksum")
        checksum = base64.b64decode(lines.pop()[1:], validate=True)
    if not lines or any(not re.fullmatch(r"[A-Za-z0-9+/]+={0,2}", line) for line in lines):
        raise PackagingError("Invalid armored public key payload")
    try:
        data = base64.b64decode("".join(lines), validate=True)
    except ValueError as error:
        raise PackagingError("Invalid armored public key") from error
    if checksum is not None:
        crc = 0xB704CE
        for byte in data:
            crc ^= byte << 16
            for _ in range(8):
                crc = (crc << 1) ^ (0x1864CFB if crc & 0x800000 else 0)
        if crc.to_bytes(3, "big") != checksum:
            raise PackagingError("Armored repository public key checksum mismatch")
    offset = 0
    fingerprint = None
    while offset < len(data):
        header = data[offset]
        offset += 1
        if not header & 0x80:
            raise PackagingError("Invalid OpenPGP packet")
        if header & 0x40:
            tag = header & 0x3f
            if offset >= len(data):
                raise PackagingError("Truncated OpenPGP packet length")
            first = data[offset]
            offset += 1
            if first < 192:
                length = first
            elif first < 224:
                if offset >= len(data):
                    raise PackagingError("Truncated OpenPGP packet length")
                length = ((first - 192) << 8) + data[offset] + 192
                offset += 1
            elif first == 255:
                if offset + 4 > len(data):
                    raise PackagingError("Truncated OpenPGP packet length")
                length = int.from_bytes(data[offset:offset + 4], "big")
                offset += 4
            else:
                raise PackagingError("Partial OpenPGP lengths not accepted")
        else:
            tag = (header >> 2) & 15
            length_type = header & 3
            if length_type == 3:
                raise PackagingError("Indeterminate OpenPGP length")
            count = (1, 2, 4)[length_type]
            if offset + count > len(data):
                raise PackagingError("Truncated OpenPGP packet length")
            length = int.from_bytes(data[offset:offset + count], "big")
            offset += count
        if not length or length > len(data) - offset:
            raise PackagingError("Empty or truncated repository public key packet")
        packet = data[offset:offset + length]
        offset += length
        if tag == 6:
            if fingerprint is not None:
                raise PackagingError("Multiple primary repository public keys are forbidden")
            if len(packet) < 6 or packet[0] != 4 or len(packet) > 65535:
                raise PackagingError("Unsupported repository key format")
            fingerprint = hashlib.sha1(b"\x99" + len(packet).to_bytes(2, "big") + packet).hexdigest().upper()
        elif fingerprint is None or tag not in (2, 13, 14, 17):
            # Do not unpack compressed/special packets: managers may import keys
            # hidden in them. Retain only public-certificate packet types.
            raise PackagingError(f"Forbidden repository public key packet: {tag}")
        elif tag == 14 and (len(packet) < 6 or packet[0] != 4 or len(packet) > 65535):
            raise PackagingError("Unsupported repository public subkey format")
    if fingerprint is None:
        raise PackagingError("Repository key has no primary public key")
    return fingerprint


def kernel_packages(target, facts):
    release = facts.get("kernel", "")
    if not re.fullmatch(r"[A-Za-z0-9_.+~-]+", release):
        raise PackagingError("Invalid running kernel release")
    manager = target["manager"]
    if facts.get("kernel_headers_match"):
        return []
    if manager == "pacman":
        pkgbase = facts.get("kernel_pkgbase", "")
        if not re.fullmatch(r"linux[A-Za-z0-9+_.-]*", pkgbase):
            raise PackagingError("Running kernel pkgbase unavailable; never guess linux-headers on CachyOS")
        return [f"{pkgbase}-headers"]
    if manager == "apt":
        return [f"linux-headers-{release}"]
    if manager == "dnf":
        if "debug" in release:
            raise PackagingError("Debug kernels are not supported; no silent kernel change")
        return [f"kernel-devel-{release}"]
    flavor = facts.get("kernel_flavor", "")
    version = facts.get("kernel_package_version", "")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", flavor) or not re.fullmatch(r"[A-Za-z0-9_.+~-]+", version):
        raise PackagingError("Cannot resolve exact installed openSUSE kernel version/flavor")
    return [f"kernel-{flavor}-devel={version}", f"kernel-devel={version}"]
