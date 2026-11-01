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


def target_for(release, catalog=None, today=None):
    catalog = catalog or load_targets()
    for target in catalog["targets"]:
        if target["os_id"] != release.get("ID"):
            continue
        if not target.get("rolling") and release.get("VERSION_ID") not in target["version_ids"]:
            continue
        if "support_until" in target and (today or date.today()) >= date.fromisoformat(target["support_until"]):
            raise PackagingError(f"Target {target['id']} has expired; rebuild against a maintained Fedora release")
        if target.get("codename") and release.get("VERSION_CODENAME", target["codename"]) != target["codename"]:
            raise PackagingError("Distribution codename/version mismatch")
        return target
    raise PackagingError(f"Unsupported target: {release.get('ID')} {release.get('VERSION_ID', '')}")


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
    commands = {
        "apt": ["apt-cache", "policy", name],
        "pacman": ["pacman", "-Si", "--", name],
        "dnf": ["dnf", "repoquery", "--available", "--latest-limit=1", "--qf", "%{version}-%{release}", name],
        "zypper": ["zypper", "--xmlout", "search", "--details", "--match-exact", "--type", "package", name],
    }
    if manager == "zypper" and "(" in name:
        commands[manager].insert(-1, "--provides")
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
        versions = [item.get("edition") for item in solvables if item.get("name") == name or "(" in name]
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
            version = query(target["manager"], candidate)
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
    try:
        value = json.loads(output)
    except (ValueError, UnicodeError):
        return False, "CLI did not emit JSON"
    if not isinstance(value, dict) or value.get("api_version") != "1.6":
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
            if not isinstance(device.get(key), str) or not re.fullmatch(r"(?:0x)?[0-9a-fA-F]{4}", device[key]):
                return False, f"Invalid {key}"
        for key in ("capabilities",):
            caps = device.get(key)
            if not isinstance(caps, list) or len(caps) > 128 or not all(isinstance(cap, str) for cap in caps):
                return False, f"Missing/invalid {key} in the verified JSON schema"
    return True, f"CLI schema {value['api_version']} verified; hardware support is unchanged"


def probe_headset(executable):
    if not executable:
        return {"configured": False, "reason": "CLI is not installed"}
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
        return {"configured": False, "path": str(executable), "reason": str(error)}
    good, reason = headset_contract(output)
    # Upstream 4.2.0 outputs a valid empty inventory and exits 1 when no headset
    # is attached. That verifies the software contract, not a battery reading.
    no_devices = good and json.loads(output).get("device_count") == 0
    if (result.returncode != 0 and not (result.returncode == 1 and no_devices)) or errors.strip():
        good, reason = False, "CLI probe failed or reported stderr; no confirmed battery samples"
    return {"configured": good, "path": str(executable), "reason": reason}


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
    """Fingerprint the v4 primary key; package managers still verify signatures."""
    text = armored.decode("ascii")
    match = re.search(r"-----BEGIN PGP PUBLIC KEY BLOCK-----\s*\n(.*?)-----END PGP PUBLIC KEY BLOCK-----", text, re.S)
    if not match:
        raise PackagingError("Expected an ASCII-armored repository public key")
    lines = [line.strip() for line in match[1].splitlines()]
    payload = "".join(line for line in lines if line and not line.startswith("=") and ":" not in line)
    try:
        data = base64.b64decode(payload, validate=True)
    except ValueError as error:
        raise PackagingError("Invalid armored public key") from error
    offset = 0
    while offset < len(data):
        header = data[offset]
        offset += 1
        if not header & 0x80:
            raise PackagingError("Invalid OpenPGP packet")
        if header & 0x40:
            tag = header & 0x3f
            first = data[offset]
            offset += 1
            if first < 192:
                length = first
            elif first < 224:
                length = ((first - 192) << 8) + data[offset] + 192
                offset += 1
            elif first == 255:
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
            length = int.from_bytes(data[offset:offset + count], "big")
            offset += count
        packet = data[offset:offset + length]
        offset += length
        if len(packet) != length:
            raise PackagingError("Truncated repository public key")
        if tag == 6:
            if not packet or packet[0] != 4 or len(packet) > 65535:
                raise PackagingError("Unsupported repository key format")
            return hashlib.sha1(b"\x99" + len(packet).to_bytes(2, "big") + packet).hexdigest().upper()
    raise PackagingError("Repository key has no primary public key")


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
