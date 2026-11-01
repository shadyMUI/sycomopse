# SPDX-License-Identifier: GPL-3.0-or-later

import ast
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parents[1]
PRIVATE = HERE / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))
from bootstrap import ARCHIVE_SHA256, COMMIT, verify_vendor

spec = importlib.util.spec_from_file_location("private_fetch_engine", HERE / "fetch-engine.py")
fetch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch)


def archive_fixture(extra=()):
    root = "input-remapper-fixture"
    files = {
        "LICENSE": b"Fixture GPL license; test bytes, not a production license",
        "pyproject.toml": b'version = "2.2.1"\n',
        "inputremapper/__init__.py": b"",
        "inputremapper/installation_info.py": b'VERSION = "2.2.1"\n',
        "inputremapper/injection/injector.py": b"# Test archive fixture only\n",
        "po/it_IT.po": b"# test translation\n",
    }
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, data in files.items():
            info = tarfile.TarInfo(root + "/" + name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
        for info in extra:
            archive.addfile(info, io.BytesIO(b""))
    data = output.getvalue()
    lock = json.loads((HERE / "source-lock.json").read_text(encoding="utf-8"))
    lock.update(archiveRoot=root, archiveSha256=hashlib.sha256(data).hexdigest())
    return data, lock


class StagingTests(unittest.TestCase):
    def setUp(self):
        (HERE / ".cache").mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=HERE / ".cache", prefix="test-stage-")
        self.directory = Path(self.temporary.name)
        self.data, self.lock = archive_fixture()

    def tearDown(self):
        self.temporary.cleanup()

    def test_pinned_lock_matches_verified_archive_and_version(self):
        lock = json.loads((HERE / "source-lock.json").read_text(encoding="utf-8"))
        self.assertEqual(lock["commit"], COMMIT)
        self.assertEqual(lock["archiveSha256"], ARCHIVE_SHA256)
        self.assertEqual(lock["version"], "2.2.1")
        self.assertEqual(lock["patches"], [])
        cached = HERE / ".cache" / f"input-remapper-{COMMIT}.tar.gz"
        if cached.is_file():
            files = fetch.verified_members(cached.read_bytes(), lock)
            self.assertEqual(len([name for name in files if name.startswith("inputremapper/")]), 106)
            self.assertIn("LICENSE", files)

    def test_checksum_mismatch_is_rejected_before_staging(self):
        with self.assertRaises(ValueError):
            fetch.stage(self.directory / "rootfs", self.data + b"tampered", self.lock)
        self.assertFalse((self.directory / "rootfs").exists())

    def test_tar_path_escape_special_files_and_engine_symlinks_are_rejected(self):
        for name, kind, link in (("input-remapper-fixture/../outside", tarfile.REGTYPE, ""), ("/outside", tarfile.REGTYPE, ""), ("input-remapper-fixture/inputremapper/link.py", tarfile.SYMTYPE, "../../outside"), ("input-remapper-fixture/po/bad.po", tarfile.SYMTYPE, "../../outside"), ("input-remapper-fixture/device", tarfile.CHRTYPE, "")):
            with self.subTest(name=name):
                info = tarfile.TarInfo(name)
                info.type, info.linkname = kind, link
                data, lock = archive_fixture([info])
                with self.assertRaises(ValueError):
                    fetch.verified_members(data, lock)

    def test_upstream_local_translation_aliases_are_safely_dereferenced(self):
        info = tarfile.TarInfo("input-remapper-fixture/po/it.po")
        info.type, info.linkname = tarfile.SYMTYPE, "./it_IT.po"
        data, lock = archive_fixture([info])
        files = fetch.verified_members(data, lock)
        self.assertEqual(files["po/it.po"], files["po/it_IT.po"])

    def test_only_private_module_and_license_source_are_staged(self):
        destination = self.directory / "rootfs"
        files, manifest = fetch.stage(destination, self.data, self.lock)
        private = destination / fetch.PRIVATE_DIR
        self.assertTrue((private / "vendor/inputremapper/installation_info.py").is_file())
        self.assertEqual(manifest["files"]["inputremapper/injection/injector.py"], hashlib.sha256(files["inputremapper/injection/injector.py"]).hexdigest())
        docs = destination / fetch.DOC_DIR
        self.assertEqual((docs / "LICENSE.input-remapper").read_bytes(), files["LICENSE"])
        self.assertEqual((docs / "input-remapper-2.2.1-source.tar.gz").read_bytes(), self.data)
        self.assertFalse((destination / "usr/bin/input-remapper-service").exists())
        self.assertFalse((destination / "usr/share/dbus-1/system.d/inputremapper.Control.conf").exists())

    def test_refetch_preserves_separately_staged_private_dependencies(self):
        destination = self.directory / "rootfs"
        fetch.stage(destination, self.data, self.lock)
        dependency = destination / fetch.PRIVATE_DIR / "vendor/evdev"
        dependency.mkdir()
        marker = dependency / "separately-staged.txt"
        marker.write_text("Another packaging worker owns this native dependency", encoding="utf-8")
        fetch.stage(destination, self.data, self.lock)
        self.assertTrue(marker.is_file())

    def test_source_bundle_is_byte_reproducible_and_has_corresponding_checksums(self):
        files = fetch.verified_members(self.data, self.lock)
        first = self.directory / "first.tar.gz"
        second = self.directory / "second.tar.gz"
        first_hash = fetch.source_bundle(first, files, self.lock, self.data)
        second_hash = fetch.source_bundle(second, files, self.lock, self.data)
        self.assertEqual(first_hash, second_hash)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with tarfile.open(first, "r:gz") as archive:
            names = set(archive.getnames())
            self.assertIn("upstream/input-remapper-2.2.1-source.tar.gz", names)
            self.assertIn("peripheral-hub-remapper/fetch-engine.py", names)
            self.assertIn("peripheral-hub-remapper/rootfs/usr/lib/peripheral-hub/remapper/bridge.py", names)
            checksum_file = archive.extractfile("SOURCE-CHECKSUMS.sha256").read().decode("utf-8")
            for line in checksum_file.splitlines():
                expected, name = line.split("  ", 1)
                self.assertEqual(hashlib.sha256(archive.extractfile(name).read()).hexdigest(), expected)
            self.assertTrue(all(not name.startswith("peripheral-hub-remapper/.cache/") for name in names))
            self.assertTrue(all(member.mtime == self.lock["sourceDateEpoch"] for member in archive.getmembers()))

    def test_vendor_hash_check_detects_changed_modules_without_loading_engine(self):
        destination = self.directory / "rootfs"
        _, manifest = fetch.stage(destination, self.data, self.lock)
        private = destination / fetch.PRIVATE_DIR
        # The explicit archive fixture is trusted only by this test's stage call.
        manifest["archiveSha256"] = ARCHIVE_SHA256
        (private / "vendor/engine-lock.json").write_text(json.dumps(manifest), encoding="utf-8")
        self.assertEqual(verify_vendor(private, require_root=False), private / "vendor")
        source = private / "vendor/inputremapper/injection/injector.py"
        source.write_bytes(source.read_bytes() + b"# changed\n")
        with self.assertRaises(RuntimeError):
            verify_vendor(private, require_root=False)

    def test_vendor_refuses_unverified_extra_python_modules(self):
        destination = self.directory / "rootfs"
        _, manifest = fetch.stage(destination, self.data, self.lock)
        private = destination / fetch.PRIVATE_DIR
        manifest["archiveSha256"] = ARCHIVE_SHA256
        (private / "vendor/engine-lock.json").write_text(json.dumps(manifest), encoding="utf-8")
        (private / "vendor/inputremapper/extra.py").write_bytes(b"# unverified code\n")
        with self.assertRaises(RuntimeError):
            verify_vendor(private, require_root=False)

    def test_system_policy_owns_only_private_name_and_allows_only_exact_calls(self):
        policy = ET.parse(HERE / "rootfs/usr/share/dbus-1/system.d/io.peripheralhub.InputRemapper.conf").getroot()
        owners = [entry.attrib["own"] for entry in policy.findall("./policy/allow") if "own" in entry.attrib]
        self.assertEqual(owners, ["io.peripheralhub.InputRemapper"])
        calls = {entry.attrib["send_member"] for entry in policy.findall("./policy/allow") if entry.attrib.get("send_interface") == "io.peripheralhub.InputRemapper1"}
        self.assertEqual(calls, {"GetVersion", "Inventory", "State", "Apply", "Stop"})
        self.assertEqual(policy.find("./policy[@user='root']/allow").attrib["own"], "io.peripheralhub.InputRemapper")

    def test_wrapper_and_unit_have_private_absolute_paths_and_graceful_kill_mode(self):
        wrapper = (HERE / "rootfs/usr/lib/peripheral-hub/remapper-service").read_text(encoding="utf-8")
        self.assertIn("/usr/bin/python3 -I -B /usr/lib/peripheral-hub/remapper/bridge.py", wrapper)
        unit = (HERE / "rootfs/usr/lib/systemd/system/peripheral-hub-input-remapper.service").read_text(encoding="utf-8")
        self.assertIn("ExecStart=/usr/lib/peripheral-hub/remapper-service", unit)
        self.assertIn("BusName=io.peripheralhub.InputRemapper", unit)
        self.assertIn("KillMode=mixed", unit)
        self.assertIn("RuntimeDirectoryMode=0700", unit)
        self.assertNotIn("input-remapper.service", unit)

    def test_all_private_python_sources_parse_without_linux_imports(self):
        sources = [HERE / "fetch-engine.py", *PRIVATE.glob("*.py")]
        for source in sources:
            with self.subTest(source=source.name):
                ast.parse(source.read_bytes(), filename=str(source))
                compile(source.read_bytes(), str(source), "exec")

    def test_private_source_files_have_no_trailing_whitespace(self):
        files = [HERE / "fetch-engine.py", HERE / "source-lock.json", *PRIVATE.glob("*.py"), *Path(__file__).parent.glob("*.py")]
        for source in files:
            with self.subTest(source=source.name):
                text = source.read_text(encoding="utf-8")
                self.assertTrue(text.endswith("\n"))
                self.assertTrue(all(line == line.rstrip() for line in text.splitlines()))


if __name__ == "__main__":
    unittest.main()
