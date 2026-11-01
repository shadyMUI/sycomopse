# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Capability-only discovery. Event numbers, model names and origin hashes are not identities."""

from collections import Counter, defaultdict
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import stat


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii")).hexdigest()


def clean_text(value, limit=256):
    return "".join(character for character in str(value or "") if character.isprintable())[:limit]


def usb_port_identity(path):
    controller = re.sub(r"/usb[0-9]+(?=/|$)", "", path)
    return re.sub(r"/[0-9]+-([0-9.]+)(?=/|$)", r"/port-\1", controller)


@dataclass(frozen=True)
class RawNode:
    physical_key: str
    physical_path: str | None
    serial: str | None
    node_role: str
    name: str
    path: str
    phys: str
    origin_hash: str
    vendor_id: int
    product_id: int
    bus_type: int
    transport: str
    key_codes: tuple[int, ...]
    relative_codes: tuple[int, ...]
    sysfs_path: str
    device_number: int
    inode: int
    virtual: bool = False
    touchpad: bool = False

    @property
    def pointer(self):
        return 272 in self.key_codes and {0, 1}.issubset(self.relative_codes)


@dataclass(frozen=True)
class Node:
    id: str
    raw: RawNode

    @property
    def origin_hash(self):
        return self.raw.origin_hash

    def public(self):
        return {
            "id": self.id,
            "originHash": self.raw.origin_hash,
            "name": self.raw.name,
            "path": self.raw.path,
            "phys": self.raw.phys,
            "keyCodes": list(self.raw.key_codes),
            "relativeCodes": list(self.raw.relative_codes),
        }


@dataclass(frozen=True)
class Button:
    id: str
    label: str
    code: int
    node_id: str
    origin_hash: str

    def public(self):
        return {"id": self.id, "label": self.label, "code": self.code, "nodeId": self.node_id, "originHash": self.origin_hash}


@dataclass(frozen=True)
class Mouse:
    fingerprint: str
    identity_strength: str
    name: str
    serial: str | None
    physical_path: str | None
    vendor_id: int
    product_id: int
    transport: str
    nodes: tuple[Node, ...]
    buttons: tuple[Button, ...]

    @property
    def signature(self):
        return digest([node.raw.__dict__ for node in self.nodes])

    def public(self):
        return {
            "id": self.fingerprint,
            "fingerprint": self.fingerprint,
            "identityStrength": self.identity_strength,
            "name": self.name,
            "serial": self.serial,
            "physicalPath": self.physical_path,
            "vendorId": self.vendor_id,
            "productId": self.product_id,
            "transport": self.transport,
            "nodes": [node.public() for node in self.nodes],
            "buttons": [button.public() for button in self.buttons],
        }


def build_inventory(observations, input_codes, code_name):
    groups = defaultdict(list)
    for node in observations:
        if node.virtual or node.touchpad or not node.physical_key:
            continue
        groups[(node.physical_key, node.bus_type, node.vendor_id, node.product_id)].append(node)
    candidates = []
    for physical, nodes in groups.items():
        pointers = [node for node in nodes if node.pointer]
        # More than one pointer at an indistinguishable receiver/port is ambiguous.
        if len(pointers) != 1 or len(nodes) > 16:
            continue
        serials = {node.serial for node in nodes if node.serial}
        if len(serials) > 1 or len({node.node_role for node in nodes}) != len(nodes):
            continue
        # The frozen engine keys contexts by origin_hash; collisions cannot be guessed.
        if len({node.origin_hash for node in nodes}) != len(nodes):
            continue
        serial = next(iter(serials), None)
        first = pointers[0]
        candidates.append((physical, nodes, first, serial))
    serial_counts = Counter((first.bus_type, first.vendor_id, first.product_id, serial) for _, _, first, serial in candidates if serial)
    mice = []
    for physical, raw_nodes, first, serial in candidates:
        identity = [first.transport, first.bus_type, first.vendor_id, first.product_id]
        strong = serial is not None and serial_counts[(first.bus_type, first.vendor_id, first.product_id, serial)] == 1
        if strong:
            strength = "serial"
            identity.append(serial)
        else:
            if not first.physical_path:
                continue
            strength = "port"
            identity.append(physical[0])
        fingerprint = strength + ":" + digest(identity)
        nodes = tuple(sorted((Node("node-" + digest(raw.node_role), raw) for raw in raw_nodes), key=lambda node: node.id))
        buttons = tuple(
            Button("button-" + digest([fingerprint, node.id, code]), clean_text(code_name(code)), code, node.id, node.origin_hash)
            for node in nodes for code in node.raw.key_codes if code in input_codes and code != 0
        )
        if not buttons or len(buttons) > 768:
            continue
        mice.append(Mouse(fingerprint, strength, first.name, serial, first.physical_path, first.vendor_id, first.product_id, first.transport, nodes, buttons))
    counts = Counter(mouse.fingerprint for mouse in mice)
    return tuple(sorted((mouse for mouse in mice if counts[mouse.fingerprint] == 1), key=lambda mouse: mouse.fingerprint))


class LinuxDiscovery:
    def __init__(self, evdev, get_device_hash):
        self.evdev = evdev
        self.get_device_hash = get_device_hash
        ecodes = evdev.ecodes
        self.input_codes = frozenset(code for code in ecodes.keys if type(code) is int and 0 < code <= 0x2FF) | frozenset(range(0x110, 0x120))

    def code_name(self, code):
        names = self.evdev.ecodes.bytype.get(1, {}).get(code, "Mouse button " + str(code))
        return names[0] if isinstance(names, (list, tuple)) else names

    @staticmethod
    def _text(path):
        try:
            return path.read_text(encoding="utf-8").strip()[:1024]
        except (OSError, UnicodeError):
            return ""

    def inspect(self, device):
        path = str(device.path)
        if not re.fullmatch(r"/dev/input/event[0-9]+", path):
            return None
        metadata = os.lstat(path)
        opened = os.fstat(device.fd)
        if not stat.S_ISCHR(metadata.st_mode) or metadata.st_uid != 0 or (metadata.st_ino, metadata.st_rdev) != (opened.st_ino, opened.st_rdev):
            return None
        sysfs = (Path("/sys/class/input") / Path(path).name / "device").resolve(strict=True)
        if not str(sysfs).startswith("/sys/devices/"):
            return None
        name = clean_text(device.name)
        phys = clean_text(device.phys)
        virtual = "/virtual/" in str(sysfs) or name.lower().startswith(("input-remapper", "peripheral-hub-remapper")) or phys.lower().startswith(("input-remapper", "peripheral-hub-remapper"))
        capabilities = device.capabilities(absinfo=False)
        keys = tuple(sorted(set(capabilities.get(1, ()))))
        relative = tuple(sorted(set(capabilities.get(2, ()))))
        absolute = set(capabilities.get(3, ()))
        properties = set(device.input_props())
        touchpad = 53 in absolute or 0x145 in keys or bool(properties & {2, 3, 5}) or 0x140 in keys or 0x14B in keys
        bus = int(device.info.bustype)
        vendor = int(device.info.vendor)
        product = int(device.info.product)
        transport = {3: "usb", 5: "bluetooth", 17: "i8042", 24: "i2c", 28: "spi"}.get(bus, "bus-" + str(bus))
        usb = None
        hid = None
        hid_properties = {}
        for ancestor in (sysfs, *sysfs.parents):
            if ancestor == Path("/sys/devices"):
                break
            if hid is None:
                uevent = self._text(ancestor / "uevent")
                props = dict(line.split("=", 1) for line in uevent.splitlines() if "=" in line)
                if "HID_ID" in props:
                    hid = ancestor
                    hid_properties = props
            if (ancestor / "idVendor").is_file() and (ancestor / "idProduct").is_file():
                usb = ancestor
                break
        base_phys = re.sub(r"/input[0-9]+(?:/.*)?$", "", phys)
        serial = clean_text(device.uniq or hid_properties.get("HID_UNIQ")) or None
        if usb is not None and bus == 3:
            try:
                same_usb_identity = int(self._text(usb / "idVendor"), 16) == vendor and int(self._text(usb / "idProduct"), 16) == product
            except ValueError:
                return None
            product_name = self._text(usb / "product").lower() + " " + name.lower()
            receiver = not same_usb_identity or any(word in product_name for word in ("receiver", "dongle", "transceiver", "adapter"))
            # A shared receiver is not a strong mouse serial. Without a concrete
            # logical HID serial, do not include its keyboard-like companion nodes.
            if receiver and not serial and not (272 in keys and {0, 1}.issubset(relative)):
                return None
            if not serial and same_usb_identity and not receiver:
                serial = clean_text(self._text(usb / "serial")) or None
            physical_path = str(usb)
            endpoint = str(sysfs.relative_to(usb))
            interface = next((match[1] for part in sysfs.parts if (match := re.fullmatch(r"[0-9]+-[0-9.]+:([0-9]+\.[0-9]+)", part))), "")
            # A receiver serial is not the identity of a different logical HID product.
            physical_key = json.dumps([usb_port_identity(physical_path), serial if receiver else None], separators=(",", ":"))
        elif hid is not None and base_phys:
            physical_path = str(hid)
            endpoint = str(sysfs.relative_to(hid))
            interface = "hid"
            physical_key = json.dumps([base_phys, serial], separators=(",", ":"))
        else:
            return None
        if serial and (serial.lower() in ("unknown", "none", "null") or not serial.strip("0:- ")):
            serial = None
        endpoint = re.sub(r"[0-9]+-[0-9.]+:([0-9]+\.[0-9]+)", r"interface:\1", endpoint)
        endpoint = re.sub(r"([0-9A-Fa-f]{4}:[0-9A-Fa-f]{4}:[0-9A-Fa-f]{4})\.[0-9A-Fa-f]+", r"\1", endpoint)
        endpoint = re.sub(r"(?:^|/)input/input[0-9]+(?:/event[0-9]+)?$", "", endpoint)
        phys_role = phys[len(base_phys):] if base_phys else ""
        role = json.dumps([interface, endpoint, phys_role], separators=(",", ":"))
        return RawNode(physical_key, physical_path, serial, role, name, path, phys, str(self.get_device_hash(device)), vendor, product, bus, transport, keys, relative, str(sysfs), opened.st_rdev, opened.st_ino, virtual, touchpad)

    def scan(self):
        observations = []
        paths = sorted(self.evdev.list_devices())
        if len(paths) > 512:
            raise OSError("Input inventory exceeds device limit")
        for path in paths:
            device = None
            try:
                device = self.evdev.InputDevice(path)
                node = self.inspect(device)
                if node is not None:
                    observations.append(node)
            except (OSError, ValueError, AttributeError):
                continue
            finally:
                if device is not None:
                    device.close()
        return build_inventory(observations, self.input_codes, self.code_name)
