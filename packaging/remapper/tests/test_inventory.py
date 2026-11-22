# SPDX-License-Identifier: GPL-3.0-or-later
"""Discovery against fake sysfs, udev records and evdev descriptors only."""

from contextlib import ExitStack
from pathlib import Path
import stat
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

PRIVATE = Path(__file__).resolve().parents[1] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))

from inventory import LinuxDiscovery
from fakes import KEYS, FakeFilesystem, FakeInput


class DiscoveryTests(unittest.TestCase):
    def setUp(self):
        self.usb = Path("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3")
        self.hid = self.usb / "1-3:1.0/0003:1532:0084.0001"
        self.sysfs = self.hid / "input/input10"
        self.attributes = {
            self.usb / "idVendor": "1532",
            self.usb / "idProduct": "0084",
            self.usb / "product": "USB Receiver",
            self.usb / "serial": "shared-receiver-serial",
            self.hid / "uevent": "HID_ID=0003:00001532:00000084\nHID_UNIQ=",
        }
        self.fs = FakeFilesystem("unused", b"")
        self.fs.lstat = lambda path: self.fs.stat(str(path))
        for directory in ("/run", "/run/udev", "/run/udev/data"):
            self.fs.add(directory, stat.S_IFDIR | 0o755, 0)
        self.record = "/run/udev/data/+input:input10"
        self.udev()
        self.source = FakeInput(codes=(272, 273, 274, 275))
        self.source.fd = 100
        self.source.phys = "usb-controller-3/input0"
        self.source.uniq = "logical-mouse-serial"
        self.source.input_props = lambda: []
        self.source.info = SimpleNamespace(bustype=3, vendor=0x1532, product=0x0084)
        self.source.close = lambda: None
        self.fs.add(self.source.path, stat.S_IFCHR | 0o660, 0)
        self.fs.nodes[self.source.path]["stat"].st_rdev = 13 * 256 + 5
        self.fs.descriptors[self.source.fd] = self.source.path
        self.evdev = SimpleNamespace(
            ecodes=SimpleNamespace(keys=dict.fromkeys(KEYS), bytype={1: {}}),
            list_devices=lambda: [self.source.path], InputDevice=lambda path: self.source,
        )
        self.discovery = LinuxDiscovery(self.evdev, lambda device: device.origin)
        self.patches = ExitStack()
        self.addCleanup(self.patches.close)
        self.patches.enter_context(patch("inventory.os", self.fs))
        self.patches.enter_context(patch.object(Path, "resolve", lambda path, **kwargs: self.sysfs))
        self.patches.enter_context(patch.object(Path, "is_file", lambda path: path in self.attributes))
        self.patches.enter_context(patch.object(LinuxDiscovery, "_text", staticmethod(lambda path: self.attributes.get(path, ""))))

    def udev(self, seat=None, extra=b"", tags=b"G:seat\nQ:seat\nV:1\n"):
        data = b"I:123456\n" + tags
        if seat is not None:
            data += b"E:ID_SEAT=" + seat.encode("utf-8") + b"\n"
        self.fs.add(self.record, stat.S_IFREG | 0o644, 0, data + extra)

    def inspect(self):
        return self.discovery.inspect(self.source)

    def test_initialized_parent_defaults_to_logind_seat0(self):
        node = self.inspect()
        self.assertIsNotNone(node)
        self.assertEqual(getattr(node, "seat", None), "seat0")

    def test_foreign_or_invalid_seat_is_not_discovered_even_with_unique_serial(self):
        for seat in ("seat1", "seat-other", "seat0 ", "../seat0", "seat0\x00"):
            with self.subTest(seat=seat):
                self.udev(seat)
                self.assertIsNone(self.inspect())
                self.assertEqual(self.discovery.scan(), ())

    def test_missing_or_uninitialized_udev_parent_fails_closed(self):
        for data in (None, b"", b"G:seat\nQ:seat\nV:1\n", b"I:0\nG:seat\n", b"I:123\n", b"I:123\nG:seat\nQ:uaccess\nV:1\n"):
            with self.subTest(data=data):
                if data is None:
                    self.fs.nodes.pop(self.record, None)
                else:
                    self.fs.add(self.record, stat.S_IFREG | 0o644, 0, data)
                self.assertIsNone(self.inspect())

    def test_ambiguous_truncated_and_unknown_version_records_fail_closed(self):
        for extra in (b"E:ID_SEAT=seat1\nE:ID_SEAT=seat0\n", b"I:123\n", b"V:2\n", b"E:ID_SEAT=seat0", b"E:ID_SEAT=\xff\n"):
            with self.subTest(extra=extra):
                self.udev(extra=extra)
                self.assertIsNone(self.inspect())

    def test_udev_record_and_ancestors_must_be_root_owned_and_not_writable(self):
        for path, owner, mode in ((self.record, 1000, stat.S_IFREG | 0o644), (self.record, 0, stat.S_IFREG | 0o666), (self.record, 0, stat.S_IFIFO | 0o644), ("/run/udev", 1000, stat.S_IFDIR | 0o755), ("/run/udev/data", 0, stat.S_IFDIR | 0o777)):
            with self.subTest(path=path, owner=owner, mode=mode):
                metadata = self.fs.nodes[path]["stat"]
                original = metadata.st_uid, metadata.st_mode
                metadata.st_uid, metadata.st_mode = owner, mode
                try:
                    self.assertIsNone(self.inspect())
                finally:
                    metadata.st_uid, metadata.st_mode = original

    def test_record_and_directory_symlinks_are_rejected(self):
        for path in ("/run", "/run/udev/data", self.record):
            with self.subTest(path=path):
                self.fs.nodes[path]["symlink"] = True
                try:
                    self.assertIsNone(self.inspect())
                finally:
                    self.fs.nodes[path]["symlink"] = False

    def test_udev_changes_during_single_descriptor_read_fail_closed(self):
        self.fs.after_read = lambda: setattr(self.fs.nodes[self.record]["stat"], "st_mtime_ns", 99)
        self.assertIsNone(self.inspect())

    def test_oversized_udev_record_is_rejected_before_read(self):
        self.fs.nodes[self.record]["stat"].st_size = 65537
        self.assertIsNone(self.inspect())
        self.assertEqual(self.fs.read_calls, 0)

    def test_safe_record_read_closes_every_udev_descriptor(self):
        original = dict(self.fs.descriptors)
        self.assertIsNotNone(self.inspect())
        self.assertEqual(self.fs.descriptors, original)

    def test_seat_is_read_from_input_parent_not_environment_or_event_number(self):
        self.udev("seat1")
        self.fs.add("/run/udev/data/c13:5", stat.S_IFREG | 0o644, 0, b"I:1\nG:seat\nE:ID_SEAT=seat0\n")
        self.assertIsNone(self.inspect())

    def test_legacy_current_tags_and_explicit_empty_seat_match_logind(self):
        self.udev("", tags=b"G:seat\n")
        node = self.inspect()
        self.assertIsNotNone(node)
        self.assertEqual(getattr(node, "seat", None), "seat0")

    def test_placeholder_serials_never_authorize_receiver_keyboard_companions(self):
        self.source.codes = (30, 164)
        self.source.capabilities = lambda **kwargs: {1: list(self.source.codes)}
        for serial in (None, "unknown", " Unknown ", "none", "null", "0", "00000000", "00:00-00"):
            with self.subTest(serial=serial):
                self.source.uniq = serial
                self.assertIsNone(self.inspect())
        self.source.uniq = "concrete-logical-mouse"
        self.assertIsNotNone(self.inspect())

    def test_placeholder_serials_are_normalized_before_physical_key(self):
        nodes = []
        for serial in (None, "unknown", "NONE", "null", "0", "00000000", "00:00-00"):
            self.source.uniq = serial
            node = self.inspect()
            self.assertIsNotNone(node)
            self.assertIsNone(node.serial)
            nodes.append(node)
        self.assertEqual(len({node.physical_key for node in nodes}), 1)

    def test_hid_and_direct_usb_serials_are_normalized_before_identity(self):
        self.source.uniq = "unknown"
        self.attributes[self.usb / "product"] = "Direct Mouse"
        self.attributes[self.usb / "serial"] = "00000000"
        self.assertIsNone(self.inspect().serial)
        self.attributes[self.usb / "serial"] = "direct-unit"
        self.assertEqual(self.inspect().serial, "direct-unit")
        self.attributes.pop(self.usb / "idVendor")
        self.attributes.pop(self.usb / "idProduct")
        first = self.inspect()
        self.source.uniq = None
        second = self.inspect()
        self.assertEqual(first.physical_key, second.physical_key)


if __name__ == "__main__":
    unittest.main()
