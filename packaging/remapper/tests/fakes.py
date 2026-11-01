# SPDX-License-Identifier: GPL-3.0-or-later
"""Explicit dependency fakes for tests only. Production imports none of these."""

from dataclasses import replace
import hashlib
import json
from pathlib import PurePosixPath
import stat
from types import SimpleNamespace

from inventory import RawNode, build_inventory
from strict import EngineStatus
from validation import MOUSE_OUTPUT_CODES


UID = 1000
PROFILE = "5ebd520a-ff81-4cd8-9a24-f0381d971d39"
KEYS = frozenset((1, 28, 29, 30, 31, 42, 46, 54, 56, 97, 100, 113, 125, 126, 164))
CONFIG = "/home/test/.config/peripheral-hub/remapping"
CALL = {"sender": ":1.10"}


def raw_node(**changes):
    node = RawNode("usb-port-controller-3", "/sys/devices/pci/usb1/1-3", "serial-unique", "interface1.0/input0", "Physical Mouse", "/dev/input/event5", "usb-controller-3/input0", "a" * 32, 0x1532, 0x0084, 3, "usb", (272, 273, 274, 275), (0, 1, 8), "/sys/devices/pci/usb1/1-3/1-3:1.0/input/input10", 13 * 256 + 5, 101)
    return replace(node, **changes)


def mouse_fixture(observations=None):
    return build_inventory(observations or [raw_node()], KEYS | MOUSE_OUTPUT_CODES, lambda code: "CODE_" + str(code))[0]


def payload_for(mouse, output=None, code=275, uid=UID, bindings=None):
    button = next(button for button in mouse.buttons if button.code == code)
    return {
        "schemaVersion": 1, "ownerUid": uid, "profileId": PROFILE,
        "fingerprint": mouse.fingerprint,
        "bindings": bindings if bindings is not None else [{"input": {"type": 1, "code": code, "nodeId": button.node_id, "originHash": button.origin_hash}, "output": output or {"kind": "key", "code": 30}}],
    }


def encoded(payload):
    data = json.dumps(payload, ensure_ascii=True, separators=(",", ":")).encode("utf-8")
    return stem_for(data, payload["ownerUid"]), data


def stem_for(data, uid=UID, profile=PROFILE):
    return f"ph-{uid}-{profile}-{hashlib.sha256(data).hexdigest()}"


def session(uid=UID, **changes):
    value = {"uid": uid, "path": f"/org/freedesktop/login1/session/c{uid}", "active": True, "remote": False, "type": "wayland", "class": "user", "state": "active", "seat": "seat0"}
    value.update(changes)
    return value


class FakeDiscovery:
    def __init__(self, mice):
        self.mice = tuple(mice)
        self.calls = 0

    def scan(self):
        self.calls += 1
        return self.mice


class FakeReader:
    def __init__(self):
        self.files = {}
        self.reads = []
        self.after_read = None

    def put(self, payload):
        stem, data = encoded(payload)
        self.files[stem] = data
        return stem

    def read(self, config, stem, uid):
        self.reads.append((config, stem, uid))
        if self.after_read:
            self.after_read()
        return self.files[stem]


class FakeHandle:
    def __init__(self, status):
        self.current = status
        self.stops = 0
        self.stop_error = False

    def status(self):
        return self.current

    def stop(self):
        self.stops += 1
        if self.stop_error:
            raise OSError("Injected shutdown failure")
        self.current = EngineStatus("STOPPED")
        return self.current


class FakeEngine:
    keyboard_codes = KEYS
    mouse_codes = MOUSE_OUTPUT_CODES

    def __init__(self):
        self.compiled = []
        self.handles = []
        self.next_state = EngineStatus("RUNNING", verified=True)
        self.compile_error = False
        self.after_start = None

    def compile(self, mouse, preset):
        if self.compile_error:
            raise ValueError("Injected compilation failure")
        self.compiled.append(preset)
        return preset

    def start(self, mouse, compiled, validated):
        handle = FakeHandle(self.next_state)
        self.handles.append(handle)
        if self.after_start:
            self.after_start()
        return handle

    def stop_all(self):
        raise AssertionError("External/stock services must never be stopped")


class FakeInput:
    def __init__(self, origin="a" * 32, codes=(275,), fail_grab=False, pressed=(), events=()):
        self.origin = origin
        self.codes = tuple(codes)
        self.fail_grab = fail_grab
        self.pressed = pressed
        self.events = list(events)
        self.grabs = self.ungrabs = 0
        self.grabbed = False
        self.name = "Test mouse source"
        self.path = "/dev/input/event5"

    def capabilities(self, absinfo=False):
        return {1: list(self.codes), 2: [0, 1, 8]}

    def grab(self):
        self.grabs += 1
        if self.fail_grab:
            raise OSError("Injected exclusive-grab conflict")
        self.grabbed = True

    def ungrab(self):
        self.ungrabs += 1
        self.grabbed = False

    def leds(self):
        return []

    def active_keys(self):
        return list(self.pressed)

    def read_one(self):
        return self.events.pop(0) if self.events else None


class FakeFilesystem:
    O_RDONLY = 0
    O_DIRECTORY = 0x10000
    O_NOFOLLOW = 0x20000
    O_CLOEXEC = 0x40000
    O_NONBLOCK = 0x800

    def __init__(self, stem, data, uid=UID):
        self.nodes = {}
        self.descriptors = {}
        self.positions = {}
        self.opens = []
        self.read_calls = 0
        self.after_read = None
        self.next_fd = 3
        for path, owner, mode in (("/", 0, 0o755), ("/home", 0, 0o755), ("/home/test", uid, 0o700), ("/home/test/.config", uid, 0o755), ("/home/test/.config/peripheral-hub", uid, 0o700), (CONFIG, uid, 0o700), (CONFIG + "/presets", uid, 0o700)):
            self.add(path, stat.S_IFDIR | mode, owner)
        self.file_path = CONFIG + "/presets/" + stem + ".json"
        self.add(self.file_path, stat.S_IFREG | 0o600, uid, data)

    def add(self, path, mode, uid, data=b""):
        metadata = SimpleNamespace(st_mode=mode, st_uid=uid, st_nlink=1, st_size=len(data), st_dev=1, st_ino=len(self.nodes) + 1, st_mtime_ns=1, st_ctime_ns=1)
        self.nodes[path] = {"stat": metadata, "data": data, "symlink": False}

    def open(self, path, flags, dir_fd=None):
        if dir_fd is not None:
            path = str(PurePosixPath(self.descriptors[dir_fd]) / path)
        self.opens.append((path, flags, dir_fd))
        node = self.nodes.get(path)
        if node is None or (node["symlink"] and flags & self.O_NOFOLLOW):
            raise OSError("Injected missing/symlink path")
        if flags & self.O_DIRECTORY and not stat.S_ISDIR(node["stat"].st_mode):
            raise OSError("Injected non-directory path")
        descriptor = self.next_fd
        self.next_fd += 1
        self.descriptors[descriptor] = path
        self.positions[descriptor] = 0
        return descriptor

    def fstat(self, descriptor):
        return SimpleNamespace(**self.nodes[self.descriptors[descriptor]]["stat"].__dict__)

    def read(self, descriptor, size):
        self.read_calls += 1
        path = self.descriptors[descriptor]
        data = self.nodes[path]["data"]
        position = self.positions[descriptor]
        self.positions[descriptor] += min(size, len(data) - position)
        if self.after_read:
            self.after_read()
        return data[position:position + size]

    def close(self, descriptor):
        del self.descriptors[descriptor]
        del self.positions[descriptor]
