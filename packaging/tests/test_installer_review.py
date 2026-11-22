import io
import json
import os
import stat
import sys
import tempfile
import unittest
from contextlib import ExitStack, redirect_stderr, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import common
import install
import native
from test_native import Commands


def headset_document(count=1):
    document = common.json_file(PACKAGING / "tests/fixtures/headsetcontrol-4.2.json")
    document["devices"] = document["devices"][:count]
    document["device_count"] = count
    return document


def probe(document, uid=1000):
    output = json.dumps(document).encode()

    def command(argv, **kwargs):
        kwargs["stdout"].write(output)
        return SimpleNamespace(returncode=0)

    with patch.object(common.os, "geteuid", return_value=uid), patch.object(common.subprocess, "run", side_effect=command):
        return common.probe_headset("/fixture/headsetcontrol")


class HeadsetSchemaReviewTests(unittest.TestCase):
    def assert_rejected(self, document):
        self.assertFalse(common.headset_contract(json.dumps(document).encode())[0])
        report = probe(document)
        self.assertFalse(report["configured"])
        self.assertEqual(report["user_access"], "unknown")

    def test_complete_fixture_matches_rust_fixture_without_rewriting_it(self):
        self.assertEqual(common.json_file(PACKAGING / "tests/fixtures/headsetcontrol-4.2.json"), common.json_file(PACKAGING.parent / "src-tauri/tests/hardware_headsetcontrol.json"))
        self.assertTrue(common.headset_contract(json.dumps(headset_document(3)).encode())[0])
        report = probe(headset_document(2))
        self.assertTrue(report["configured"])
        self.assertEqual(report["user_access"], "verified")

    def test_missing_root_name_or_device_name_is_never_verified(self):
        for field, root in (("name", True), ("device", False)):
            document = headset_document()
            (document if root else document["devices"][0]).pop(field)
            with self.subTest(field=field):
                self.assert_rejected(document)

    def test_names_status_and_error_shapes_match_the_rust_consumer(self):
        invalid = []
        for name in (None, 1, "AnotherTool"):
            document = headset_document()
            document["name"] = name
            invalid.append(document)
        for changes in ({"device": None}, {"device": 1}, {"status": None}, {"status": "invented"}, {"errors": None}, {"errors": []}, {"errors": ""}, {"errors": {"battery": None}}, {"errors": {"battery": 1}}):
            document = headset_document()
            document["devices"][0].update(changes)
            invalid.append(document)
        document = headset_document()
        document["devices"][0].pop("status")
        invalid.append(document)
        for document in invalid:
            with self.subTest(document=document):
                self.assert_rejected(document)

    def test_battery_fields_are_typed_bounded_and_not_invented(self):
        for battery in (None, [], 64, {}, {"status": "invented", "level": 64}, {"status": "BATTERY_AVAILABLE"}):
            document = headset_document()
            document["devices"][0]["battery"] = battery
            with self.subTest(battery=battery):
                self.assert_rejected(document)
        for level in (None, "64", 64.5, True, -2, -1, 101):
            document = headset_document()
            document["devices"][0]["battery"]["level"] = level
            with self.subTest(level=level):
                self.assert_rejected(document)

    def test_record_limits_and_identifiers_match_rust(self):
        for field, values in (("capabilities", (None, {}, [1], ["CAP_BATTERY_STATUS"] * 129)), ("id_vendor", (None, 0x046d, "0x10000", "-1", "\x1c0x046d")), ("id_product", (None, "0x10000", "0x"))):
            for value in values:
                document = headset_document()
                document["devices"][0][field] = value
                with self.subTest(field=field, value=value):
                    self.assert_rejected(document)
        for vendor in ("046d", "0X046D", " 0x046d "):
            document = headset_document()
            document["devices"][0]["id_vendor"] = vendor
            self.assertTrue(common.headset_contract(json.dumps(document).encode())[0])
        document = headset_document()
        document["devices"] *= 257
        document["device_count"] = 257
        self.assert_rejected(document)

    def test_only_real_supported_battery_capability_samples_verify_access(self):
        for changes in ({"capabilities": []}, {"capabilities": ["CAP_SIDETONE"]}, {"id_vendor": "0xf00b"}, {"battery": {"status": "BATTERY_CHARGING", "level": -1}}):
            document = headset_document()
            document["devices"][0].update(changes)
            report = probe(document)
            self.assertTrue(report["configured"])
            self.assertEqual(report["user_access"], "unknown")
        for level in (0, 100):
            document = headset_document()
            document["devices"][0]["battery"]["level"] = level
            self.assertEqual(probe(document)["user_access"], "verified")
        self.assertEqual(probe(headset_document(), uid=0)["user_access"], "unknown")

    def test_non_utf8_and_non_finite_json_do_not_pass_the_rust_contract(self):
        text = json.dumps(headset_document())
        for data in (text.encode("utf-16"), text[:-1].encode() + b', "extra":NaN}', text[:-1].encode() + b', "extra":Infinity}', text[:-1].encode() + b', "extra":-Infinity}'):
            with self.subTest(data=data[-50:]):
                self.assertFalse(common.headset_contract(data)[0])


class NativeFailureReviewTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.owner = os.getuid()
        self.status = self.root / "native-status.json"
        self.failure = self.root / "native-failure.json"
        self.metadata = self.root / "build-target.json"
        self.previous = {"schema": 1, "format": "rpm", "engine": common.ENGINE, "status": "running", "enabled": True, "running": True, "pending": []}
        common.write_json(self.status, self.previous)
        self.target = common.target_for({"ID": "opensuse-leap", "VERSION_ID": "16.0"})
        self.host_release = {"ID": "opensuse-leap", "VERSION_ID": "16.0"}
        self.commands = Commands(enabled="enabled", active=True)

    def proof(self, phase="failed", error="native configure failed"):
        return {"schema": 1, "app": common.APP, "engine": common.ENGINE, "format": "rpm", "status": phase, "error": error if phase == "failed" else ""}

    def provenance(self, release=None):
        release = release or self.host_release
        target = common.target_for(release, allow_compatible=False)
        return {"schema": 1, "id": target["id"], "os_release": release, "image": target["image"], "image_digest": "sha256:" + "a" * 64}

    def invoke(self, provenance=None, action="configure", uid=0):
        common.write_json(self.metadata, provenance or self.provenance())
        with ExitStack() as stack:
            stack.enter_context(patch.object(native, "STATUS", self.status))
            stack.enter_context(patch.object(native, "BUILD_TARGET", self.metadata))
            stack.enter_context(patch.object(native, "regular_bytes", return_value=json_to_os_release(self.host_release)))
            stack.enter_context(patch.object(native.os, "geteuid", return_value=uid))
            stack.enter_context(patch.object(sys, "argv", [str(PACKAGING / "native.py"), action, "--format", "rpm", "--upgrade"]))
            # Ownership is simulated only for this private temporary root-state directory.
            loader = lambda path: common.native_failure(path, owner=self.owner)
            stack.enter_context(patch.object(native, "native_failure", side_effect=loader, create=True))
            if hasattr(native, "configure_checkpoint"):
                checkpoint = native.configure_checkpoint
                stack.enter_context(patch.object(native, "configure_checkpoint", side_effect=lambda *args, **kwargs: checkpoint(*args, owner=self.owner, **kwargs)))
            configure = native.configure
            remove = native.remove

            def configured(format, upgrade=False, **kwargs):
                kwargs.update(runner=self.commands, status=self.status, systemd_running=True, available=lambda name: "/fixture/" + name)
                return configure(format, upgrade, **kwargs)

            configured_call = stack.enter_context(patch.object(native, "configure", side_effect=configured))
            stack.enter_context(patch.object(native, "remove", side_effect=lambda format: remove(format, self.commands, True, lambda name: "/fixture/" + name, status=self.status)))
            stack.enter_context(redirect_stderr(io.StringIO()))
            return native.main(), configured_call

    def report(self, package=True, engine=True, headset=None):
        with ExitStack() as stack:
            stack.enter_context(patch.object(install, "STATE", self.root / "installer.json"))
            stack.enter_context(patch.object(install, "NATIVE_STATE", self.status))
            stack.enter_context(patch.object(install, "NATIVE_FAILURE", self.failure, create=True))
            stack.enter_context(patch.object(install, "HEADSET", self.root / "missing-cli"))
            stack.enter_context(patch.object(install, "native_failure", side_effect=lambda path: common.native_failure(path, owner=self.owner), create=True))
            stack.enter_context(patch.object(install, "installed", return_value={"state": "installed", "version": "0.1.0-1"} if package else None))
            stack.enter_context(patch.object(install, "run", side_effect=lambda argv, **kwargs: SimpleNamespace(returncode=0 if "--user" in argv or engine else 3)))
            return install.installation_status(self.target, {"headsetcontrol": headset or {"configured": True, "device_count": 0, "user_access": "unknown"}})

    def test_wrong_rpm_payload_before_configure_persists_failed_proof(self):
        wrong = self.provenance({"ID": "opensuse-tumbleweed", "VERSION_ID": "20261116"})
        code, configured = self.invoke(wrong)
        self.assertEqual(code, 1)
        configured.assert_not_called()
        self.assertTrue(self.failure.is_file(), "failed %post needs independent error proof")
        self.assertEqual(common.json_file(self.failure)["status"], "failed")
        self.assertEqual(common.json_file(self.status), self.previous)
        report = self.report()
        self.assertEqual(report["status"], "failed")
        self.assertTrue(report["package_installed"])
        self.assertTrue(report["private_engine_running"])
        self.assertTrue(report["pending"])

    def test_unknown_provenance_schema_cannot_clear_a_failed_configuration(self):
        for schema in (2, True, "1"):
            common.write_json(self.failure, self.proof())
            provenance = self.provenance()
            provenance["schema"] = schema
            with self.subTest(schema=schema):
                code, configured = self.invoke(provenance)
                self.assertEqual(code, 1)
                configured.assert_not_called()
                self.assertEqual(common.json_file(self.failure)["status"], "failed")
                self.assertEqual(common.json_file(self.status), self.previous)

    def test_native_status_exposes_failed_configuration_without_writing_as_user(self):
        common.write_json(self.failure, self.proof())
        before = self.failure.read_bytes()
        output = io.StringIO()
        with patch.object(native, "STATUS", self.status), patch.object(native, "native_failure", side_effect=lambda path: common.native_failure(path, owner=self.owner)), patch.object(native.os, "geteuid", return_value=1000), patch.object(sys, "argv", [str(PACKAGING / "native.py"), "status"]), redirect_stdout(output):
            self.assertEqual(native.main(), 0)
        report = json.loads(output.getvalue())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["native_status"], "running")
        self.assertEqual(self.failure.read_bytes(), before)

    def test_rpm_import_failure_is_not_hidden_by_installed_database_or_old_pid(self):
        self.commands.failures.add("--check-imports")
        code, _ = self.invoke()
        self.assertEqual(code, 1)
        self.assertTrue(self.failure.is_file())
        self.assertEqual(common.json_file(self.failure)["status"], "failed")
        self.assertEqual(common.json_file(self.status), self.previous)
        self.assertTrue(self.commands.active)
        self.assertEqual(self.report()["status"], "failed")

    def test_inflight_and_failed_proof_prevent_readiness_even_when_both_services_run(self):
        for phase, expected in (("configuring", "pending"), ("failed", "failed")):
            common.write_json(self.failure, self.proof(phase))
            with self.subTest(phase=phase):
                self.assertEqual(self.report()["status"], expected)

    def test_failed_configure_preserves_removal_marker_until_verified_success(self):
        self.previous.update(status="removed", running=False, removal={"enabled": True, "running": True, "disabled": True, "stopped": True})
        common.write_json(self.status, self.previous)
        self.commands.enabled, self.commands.active = "disabled", False
        self.commands.failures.add("--check-imports")
        self.assertEqual(self.invoke()[0], 1)
        self.assertTrue(self.failure.is_file())
        self.assertEqual(common.json_file(self.status), self.previous)
        self.commands.failures.clear()
        self.assertEqual(self.invoke()[0], 0)
        self.assertFalse(self.failure.exists())
        self.assertNotIn("removal", common.json_file(self.status))
        self.assertEqual(self.report()["status"], "running")

    def test_successful_import_but_failed_configuration_save_does_not_clear_error(self):
        common.write_json(self.failure, self.proof())
        write = native.write_json

        def save(path, record, *args):
            if Path(path) == self.status:
                raise OSError("native checkpoint save failed")
            return write(path, record, *args)

        with patch.object(native, "write_json", side_effect=save):
            self.assertEqual(self.invoke()[0], 1)
        self.assertTrue(self.failure.is_file())
        self.assertEqual(common.json_file(self.failure)["status"], "failed")
        self.assertEqual(self.report()["status"], "failed")

    def test_unknown_native_schema_and_unknown_failure_schema_are_not_overwritten(self):
        common.write_json(self.status, {"schema": 2, "removal": "unknown format"})
        self.assertEqual(self.invoke()[0], 1)
        self.assertEqual(common.json_file(self.status), {"schema": 2, "removal": "unknown format"})
        self.assertTrue(self.failure.is_file())
        common.write_json(self.status, self.previous)
        common.write_json(self.failure, {"schema": 2, "sentinel": "preserve"})
        self.assertEqual(self.invoke()[0], 1)
        self.assertEqual(common.json_file(self.failure), {"schema": 2, "sentinel": "preserve"})
        self.assertEqual(common.json_file(self.status), self.previous)
        self.assertEqual(self.report()["status"], "unknown")

    def test_unauthorized_attempt_does_not_create_or_clear_a_failure_marker(self):
        for existing in (False, True):
            if existing:
                common.write_json(self.failure, self.proof())
            before = self.failure.read_bytes() if existing else None
            code, configured = self.invoke(uid=1000)
            self.assertEqual(code, 1)
            configured.assert_not_called()
            self.assertEqual(self.failure.read_bytes() if self.failure.exists() else None, before)
            self.assertEqual(common.json_file(self.status), self.previous)

    def test_unsupported_os_removal_keeps_diagnostic_and_reinstall_requires_configure(self):
        common.write_json(self.failure, self.proof())
        before = self.failure.read_bytes()
        self.host_release = {"ID": "pop", "VERSION_ID": "26.04", "UBUNTU_CODENAME": "future"}
        self.assertEqual(self.invoke(provenance=self.provenance({"ID": "opensuse-leap", "VERSION_ID": "16.0"}), action="remove")[0], 0)
        self.assertEqual(self.failure.read_bytes(), before)
        self.assertEqual(self.report(package=False, engine=False)["status"], "removed")
        self.assertEqual(self.report(package=True, engine=True)["status"], "failed")

    def test_nonconforming_cli_cannot_make_installed_running_services_ready(self):
        for root in (True, False):
            document = headset_document()
            (document if root else document["devices"][0]).pop("name" if root else "device")
            report = self.report(headset=probe(document))
            self.assertEqual(report["status"], "pending")
            self.assertEqual(report["headsetcontrol"]["user_access"], "unknown")

    def test_failure_marker_rejects_unowned_writable_linked_and_oversized_data(self):
        common.write_json(self.failure, self.proof())
        if self.owner != 0:
            with self.assertRaises(common.PackagingError):
                common.native_failure(self.failure)
        self.failure.chmod(0o666)
        with self.assertRaises(common.PackagingError):
            common.native_failure(self.failure, owner=self.owner)
        self.failure.chmod(0o644)
        alias = self.root / "alias"
        alias.symlink_to(self.failure)
        with self.assertRaises(common.PackagingError):
            common.native_failure(alias, owner=self.owner)
        alias.unlink()
        os.link(self.failure, alias)
        with self.assertRaises(common.PackagingError):
            common.native_failure(self.failure, owner=self.owner)
        alias.unlink()
        self.failure.write_bytes(b"x" * (32768 + 1))
        with self.assertRaises(common.PackagingError):
            common.native_failure(self.failure, owner=self.owner)

    def test_absent_or_valid_marker_in_a_writable_parent_is_not_trusted(self):
        for exists in (False, True):
            if exists:
                common.write_json(self.failure, self.proof())
            self.root.chmod(0o777)
            try:
                with self.assertRaises(common.PackagingError):
                    common.native_failure(self.failure, owner=self.owner)
            finally:
                self.root.chmod(0o700)

    def test_configuration_proof_is_durable_before_provenance_checks(self):
        fsync = os.fsync
        directories = []

        def sync(descriptor):
            if stat.S_ISDIR(os.fstat(descriptor).st_mode):
                directories.append(descriptor)
            return fsync(descriptor)

        with patch.object(native.os, "fsync", side_effect=sync):
            code, configured = self.invoke(self.provenance({"ID": "opensuse-tumbleweed", "VERSION_ID": "20261116"}))
        self.assertEqual(code, 1)
        configured.assert_not_called()
        self.assertGreaterEqual(len(directories), 2, "Persist both in-flight and FAILED directory entries")


def json_to_os_release(release):
    return "".join(key + "=" + value + "\n" for key, value in release.items()).encode()


if __name__ == "__main__":
    unittest.main()
