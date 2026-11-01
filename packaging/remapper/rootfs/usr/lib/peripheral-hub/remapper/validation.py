# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""The only accepted input format. No engine DSL or user-supplied key names."""

from dataclasses import dataclass
import hashlib
import json
import os
import re
import stat
import uuid


MAX_PRESET_BYTES = 64 * 1024
MAX_BINDINGS = 128
MOUSE_OUTPUT_CODES = frozenset(range(272, 280))
MODIFIER_CODES = frozenset((29, 42, 54, 56, 97, 100, 125, 126))
STEM = re.compile(r"ph-([1-9][0-9]{0,9})-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})-([0-9a-f]{64})")
FINGERPRINT = re.compile(r"(?:serial|port):[0-9a-f]{64}")


class BackendError(Exception):
    def __init__(self, code, message):
        self.code = code
        super().__init__(message)


def invalid(message):
    raise BackendError("InvalidPreset", message)


def integer(value, minimum=0, maximum=2**32 - 1):
    return type(value) is int and minimum <= value <= maximum


def preset_identity(stem, uid=None):
    if not isinstance(stem, str) or not (match := STEM.fullmatch(stem)):
        invalid("Preset must be an immutable ph-uid-uuid-sha256 stem, without an extension")
    owner = int(match[1])
    if not integer(owner, 1) or (uid is not None and owner != uid):
        invalid("Preset stem owner does not match the caller")
    profile = str(uuid.UUID(match[2]))
    return owner, profile, match[3]


def path_components(config_dir):
    if (
        not isinstance(config_dir, str)
        or len(config_dir) > 4096
        or not config_dir.startswith("/")
        or "\\" in config_dir
        or "\0" in config_dir
    ):
        invalid("Config directory must be an absolute, canonical Linux path")
    parts = config_dir.split("/")[1:]
    if not parts or any(part in ("", ".", "..") for part in parts):
        invalid("Config directory contains traversal or non-canonical components")
    return parts


def check_directory(info, uid, private=False):
    if not stat.S_ISDIR(info.st_mode):
        invalid("Preset path component is not a directory")
    if private:
        if info.st_uid != uid or stat.S_IMODE(info.st_mode) != 0o700:
            invalid("Config and presets directories must be owned by the caller with mode 0700")
    elif info.st_uid not in (0, uid) or info.st_mode & 0o022:
        invalid("Preset path ancestor has an untrusted owner or writable permissions")


def check_file(info, uid):
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != uid
        or stat.S_IMODE(info.st_mode) not in (0o400, 0o600)
        or info.st_nlink != 1
        or not 1 <= info.st_size <= MAX_PRESET_BYTES
    ):
        invalid("Preset must be a bounded regular file, caller-owned, mode 0400/0600, without hardlinks")


def file_stamp(info):
    return tuple(getattr(info, name) for name in (
        "st_dev", "st_ino", "st_uid", "st_mode", "st_nlink", "st_size", "st_mtime_ns", "st_ctime_ns"
    ))


class SecurePresetReader:
    """Walk descriptors with O_NOFOLLOW; consume the validated inode once."""

    def __init__(self, filesystem=os):
        self.fs = filesystem

    def read(self, config_dir, stem, uid):
        _, _, digest = preset_identity(stem, uid)
        parts = path_components(config_dir)
        fs = self.fs
        descriptors = []
        try:
            directory_flags = fs.O_RDONLY | fs.O_DIRECTORY | fs.O_NOFOLLOW | fs.O_CLOEXEC
            descriptor = fs.open("/", directory_flags)
            descriptors.append(descriptor)
            check_directory(fs.fstat(descriptor), uid)
            for index, part in enumerate(parts):
                descriptor = fs.open(part, directory_flags, dir_fd=descriptor)
                descriptors.append(descriptor)
                check_directory(fs.fstat(descriptor), uid, private=index == len(parts) - 1)
            descriptor = fs.open("presets", directory_flags, dir_fd=descriptor)
            descriptors.append(descriptor)
            check_directory(fs.fstat(descriptor), uid, private=True)
            file_descriptor = fs.open(
                stem + ".json", fs.O_RDONLY | fs.O_NOFOLLOW | fs.O_CLOEXEC | fs.O_NONBLOCK, dir_fd=descriptor
            )
            descriptors.append(file_descriptor)
            before = fs.fstat(file_descriptor)
            check_file(before, uid)
            chunks = []
            size = 0
            while size <= MAX_PRESET_BYTES:
                chunk = fs.read(file_descriptor, min(16384, MAX_PRESET_BYTES + 1 - size))
                if not chunk:
                    break
                chunks.append(chunk)
                size += len(chunk)
            data = b"".join(chunks)
            after = fs.fstat(file_descriptor)
            if file_stamp(before) != file_stamp(after) or len(data) != before.st_size:
                invalid("Preset changed during its single descriptor read")
            if hashlib.sha256(data).hexdigest() != digest:
                invalid("Preset byte SHA256 does not match its immutable stem")
            return data
        except BackendError:
            raise
        except (OSError, AttributeError):
            invalid("Preset cannot be read through a safe, no-symlink path")
        finally:
            for descriptor in reversed(descriptors):
                fs.close(descriptor)


@dataclass(frozen=True)
class Action:
    kind: str
    code: int | None = None
    modifiers: tuple[int, ...] = ()
    key: int | None = None


@dataclass(frozen=True)
class Binding:
    node_id: str
    origin_hash: str
    code: int
    output: Action


@dataclass(frozen=True)
class ValidatedPreset:
    stem: str
    uid: int
    profile_id: str
    fingerprint: str
    bindings: tuple[Binding, ...]

    @property
    def effective_bindings(self):
        return tuple(binding for binding in self.bindings if binding.output.kind != "default")


def exact_fields(value, expected, message):
    if type(value) is not dict or set(value) != set(expected):
        invalid(message)


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            invalid("Duplicate JSON fields are not accepted")
        value[key] = item
    return value


class PresetValidator:
    def __init__(self, keyboard_codes, mouse_codes=MOUSE_OUTPUT_CODES):
        self.keyboard_codes = frozenset(keyboard_codes)
        self.mouse_codes = frozenset(mouse_codes) & MOUSE_OUTPUT_CODES

    def action(self, value):
        if type(value) is not dict or not isinstance(value.get("kind"), str):
            invalid("Output must be a structured MouseAction")
        kind = value["kind"]
        if kind in ("default", "disabled"):
            exact_fields(value, ("kind",), "Default and disabled have no additional output fields")
            return Action(kind)
        if kind in ("mouseButton", "key"):
            exact_fields(value, ("kind", "code"), "Key output has unsupported fields")
            allowed = self.mouse_codes if kind == "mouseButton" else self.keyboard_codes
            if not integer(value["code"], 1, 0x2FF) or value["code"] not in allowed:
                invalid("Output code is not an allowed code for the selected target")
            return Action(kind, code=value["code"])
        if kind == "chord":
            exact_fields(value, ("kind", "modifiers", "key"), "Chord output has unsupported fields")
            modifiers = value["modifiers"]
            key = value["key"]
            if (
                type(modifiers) is not list
                or not 1 <= len(modifiers) <= len(MODIFIER_CODES)
                or any(not integer(code, 1, 0x2FF) or code not in MODIFIER_CODES or code not in self.keyboard_codes for code in modifiers)
                or len(set(modifiers)) != len(modifiers)
                or not integer(key, 1, 0x2FF)
                or key not in self.keyboard_codes
                or key in MODIFIER_CODES
            ):
                invalid("Chord requires unique numeric modifiers and one allowed non-modifier key")
            return Action(kind, modifiers=tuple(modifiers), key=key)
        invalid("Arbitrary macros, scripts and unknown output kinds are forbidden")

    def validate(self, data, stem, uid, device):
        owner, profile, digest = preset_identity(stem, uid)
        if not isinstance(data, bytes) or not 1 <= len(data) <= MAX_PRESET_BYTES:
            invalid("Preset bytes exceed the size limit")
        if hashlib.sha256(data).hexdigest() != digest:
            invalid("Preset byte SHA256 does not match its immutable stem")
        try:
            payload = json.loads(data.decode("utf-8"), object_pairs_hook=unique_object, parse_constant=lambda _: invalid("Non-finite JSON values are forbidden"))
        except (UnicodeDecodeError, ValueError, RecursionError):
            invalid("Preset is not valid bounded UTF-8 JSON")
        exact_fields(payload, ("schemaVersion", "ownerUid", "profileId", "fingerprint", "bindings"), "Preset schema has unsupported or missing fields")
        if type(payload["schemaVersion"]) is not int or payload["schemaVersion"] != 1:
            invalid("Unsupported preset schemaVersion")
        if not integer(payload["ownerUid"], 1) or payload["ownerUid"] != owner:
            invalid("Preset owner is not the actual caller")
        if payload["profileId"] != profile or payload["fingerprint"] != device.fingerprint:
            invalid("Preset UUID or physical fingerprint does not match the request")
        bindings = payload["bindings"]
        if type(bindings) is not list or len(bindings) > MAX_BINDINGS:
            invalid("Preset bindings exceed the allowed limit")
        sources = {(button.node_id, button.code): button for button in device.buttons}
        validated = []
        seen = set()
        for binding in bindings:
            exact_fields(binding, ("input", "output"), "Binding schema has unsupported fields")
            source = binding["input"]
            exact_fields(source, ("type", "code", "originHash", "nodeId"), "Input schema has unsupported fields")
            if type(source["type"]) is not int or source["type"] != 1 or not integer(source["code"], 1, 0x2FF):
                invalid("Only real, bounded EV_KEY inputs are accepted")
            if not isinstance(source["nodeId"], str) or not isinstance(source["originHash"], str):
                invalid("Input identity must contain strings")
            identity = source["nodeId"], source["code"]
            actual = sources.get(identity)
            if actual is None or actual.origin_hash != source["originHash"]:
                invalid("Input source or originHash does not belong to the actual mouse node")
            if identity in seen:
                invalid("Duplicate input bindings are forbidden")
            seen.add(identity)
            validated.append(Binding(actual.node_id, actual.origin_hash, actual.code, self.action(binding["output"])))
        outputs = {(binding.node_id, binding.code): binding.output for binding in validated}
        primary_nodes = {node.id for node in device.nodes if node.raw.pointer}
        safe_left = any(
            (button.code == 272 and button.node_id in primary_nodes and outputs.get((button.node_id, button.code), Action("default")).kind == "default")
            or (outputs.get((button.node_id, button.code), Action("default")).kind == "mouseButton" and outputs[(button.node_id, button.code)].code == 272)
            for button in device.buttons
        )
        if not safe_left:
            invalid("At least one direct or passthrough primary mouse click must remain")
        return ValidatedPreset(stem, uid, profile, device.fingerprint, tuple(validated))
