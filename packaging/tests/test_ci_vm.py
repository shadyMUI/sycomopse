# SPDX-License-Identifier: MIT
"""VM protocol/security fixtures; these tests never certify a native guest run."""

import base64
import copy
import os
import signal
import subprocess
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import common
import build
from ci import vm, vm_guest


def contract(mode="readiness"):
    value = {"schema": 1, "target": vm.TARGET, "nonce": "1" * 32, "mode": mode,
             "manifest_sha256": "a" * 64 if mode == "native" else None,
             "cloud": {"url": vm.CLOUD_URL + vm.CLOUD_FILE, "sha256": "b" * 64, "signer": "C" * 40},
             "files": {"source/packaging/vm_guest.py": {"sha256": "d" * 64, "size": 0}}}
    if mode == "native":
        value["package"] = {"name": common.APP, "file": "peripheral-hub_0.1.0-1_amd64.deb", "version": "0.1.0-1", "sha256": "e" * 64}
    value["frame_sha256"] = common.digest(vm.canonical(value))
    return value


def report(expected):
    value = {key: expected[key] for key in ("nonce", "target", "mode", "manifest_sha256", "frame_sha256")}
    value.update(schema=1, executor="qemu-tcg", ready=True,
                 native_smoke="passed" if expected["mode"] == "native" else "not-run",
                 guest={"os_release": {"ID": "ubuntu", "VERSION_ID": "24.04"}, "pid1": "systemd",
                        "architecture": "x86_64", "dmi_uuid": str(uuid.UUID(expected["nonce"])),
                        "kernel": "6.8.0-fixture", "boot_id": "2" * 32, "user": {"name": "phci", "uid": 1000},
                        "versions": {"systemd": "fixture", "dbus": "fixture", "python3": "fixture"}})
    value["steps"] = list(vm.NATIVE_STEPS) if expected["mode"] == "native" else []
    commands = {
        "package": ["dpkg-query", "-W", "-f=${db:Status-Abbrev} ${Version}\n", common.APP],
        "backend": ["systemctl", "show", common.ENGINE, "--property=LoadState,UnitFileState,ActiveState,SubState,MainPID"],
        "user_manager": ["systemctl", "show", "user@1000.service", "--property=ActiveState,SubState,MainPID"],
        "agent": ["runuser", "-u", "phci", "--", "env", "XDG_RUNTIME_DIR=/run/user/1000",
                  "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus", "systemctl", "--user", "is-active", common.AGENT],
    }
    value["observations"] = []
    package = "/mnt/phci/bundle/peripheral-hub_0.1.0-1_amd64.deb"
    install = ["apt-get", "install", "--yes", "--no-install-recommends", package]
    reinstall = ["apt-get", "install", "--reinstall", "--yes", "--no-install-recommends", package]
    for index, name in enumerate(value["steps"]):
        enabled = index not in (8, 9, 11)
        active = index not in (8, 9, 10, 11, 12)
        output = {"package": "ii  0.1.0-1\n",
                  "backend": f"MainPID={888 if active else 0}\nLoadState=loaded\nActiveState={'active' if active else 'inactive'}\nSubState={'running' if active else 'dead'}\nUnitFileState={'enabled' if enabled else 'disabled'}\n",
                  "user_manager": "MainPID=777\nActiveState=active\nSubState=running\n",
                  "agent": "inactive\n" if index < 4 else "active\n"}
        transactions = []
        if index == 0:
            transactions = [{"argv": install, "returncode": 0}]
        elif index in (5, 7, 8, 9, 10):
            transactions = [{"argv": reinstall, "returncode": 0}]
        elif index in (11, 12, 13):
            transactions = [{"argv": ["apt-get", "remove", "--yes", common.APP], "returncode": 0,
                             "after_remove": {"private_payload_present": False, "backend_active": False}},
                            {"argv": install, "returncode": 0}]
        value["observations"].append({"checkpoint": name, "transactions": transactions,
            "autostart": ("f" if index < 5 else "a") * 64, "native_registry": "b" * 64,
            "payload_present": True, "recovery_present": True, "native_failure_present": False,
            "commands": {key: {"argv": argv, "returncode": 4 if key == "agent" and index < 4 else 0,
                                 "stdout": output[key], "stderr": ""} for key, argv in commands.items()}})
    return value


def bundle_fixture(root, promoted=False):
    for name in ("install.py", "system.py", "common.py", "targets.json", "app.deb", "source.tar.gz", "LICENSE"):
        (root / name).write_bytes(b"verified bytes: " + name.encode())
    manifest = {"schema": 1, "app": {"name": common.APP, "version": "0.1.0"}, "architecture": "x86_64", "init": "systemd",
        "target": {"id": "ubuntu-24.04", "os_release": {"ID": "ubuntu", "VERSION_ID": "24.04"},
                   "image": "docker.io/library/ubuntu:24.04", "image_digest": "sha256:" + "a" * 64},
        "package": {"file": "app.deb", "format": "deb", "version": "0.1.0-1"},
        "sources": [{"file": "source.tar.gz", "license": "GPL-3.0-or-later"}],
        "validation": {"software": {name: "passed" for name in vm.SOFTWARE_GATES}, "elf": "passed", "python_imports": "passed",
                       "native_smoke": "passed" if promoted else "not-run"}}
    build.write_manifest(root, manifest)
    return common.verify_bundle(root)


def frame(expected, value):
    raw = vm.canonical(value)
    header = "PHCI_VM_REPORT_V1 " + expected["nonce"] + " " + (expected["manifest_sha256"] or "none") + " " + expected["frame_sha256"]
    return (header + "\n" + base64.b64encode(raw).decode() + "\nPHCI_VM_END_V1 " + common.digest(raw) + "\n").encode()


class SignatureAndInputTests(unittest.TestCase):
    def test_malformed_or_short_digests_are_rejected(self):
        for value in (None, "", "abc", "A" * 64, "sha256:" + "a" * 64, "g" * 64):
            with self.subTest(value=value), self.assertRaises(common.PackagingError):
                vm.checked_sha(value)

    def test_signed_sums_require_exactly_one_named_noble_image(self):
        valid = ("a" * 64 + " *" + vm.CLOUD_FILE + "\n").encode()
        self.assertEqual(vm.checksum_for(valid), "a" * 64)
        for malformed in (b"", valid + valid, b"invalid  " + vm.CLOUD_FILE.encode(), b"a" * 64 + b"  wrong.img\n"):
            with self.subTest(sums=malformed), self.assertRaises(common.PackagingError):
                vm.checksum_for(malformed)

    def test_gpgv_uses_the_root_owned_apt_cloud_keyring_not_a_downloaded_key(self):
        sums = ("a" * 64 + "  " + vm.CLOUD_FILE + "\n").encode()
        def read(path, maximum, owner=None):
            return sums if str(path).endswith("SHA256SUMS") else b"fixture"
        with patch.object(vm, "regular_bytes", side_effect=read) as read_input, \
                patch.object(vm, "run", return_value=SimpleNamespace(stdout="[GNUPG:] VALIDSIG " + "B" * 40 + " 1 2 3\n")) as executed:
            self.assertEqual(vm.verify_cloud_signature(Path("/fixture")), ("a" * 64, "B" * 40))
        self.assertEqual(read_input.call_args_list[0].kwargs["owner"], 0)
        self.assertIn(str(vm.KEYRING), executed.call_args.args[0])
        self.assertNotIn("--trust-model", executed.call_args.args[0])

    def test_missing_or_bad_signature_status_fails_closed(self):
        sums = ("a" * 64 + "  " + vm.CLOUD_FILE + "\n").encode()
        for status in ("", "[GNUPG:] BADSIG bad\n", "[GNUPG:] VALIDSIG " + "B" * 40 + "\n[GNUPG:] FAILURE verify\n"):
            with self.subTest(status=status), patch.object(vm, "regular_bytes", return_value=sums), \
                    patch.object(vm, "run", return_value=SimpleNamespace(stdout=status)), self.assertRaises(common.PackagingError):
                vm.verify_cloud_signature(Path("/fixture"))

    def test_contract_tampering_or_unsupported_target_is_rejected(self):
        expected = contract()
        vm.verify_contract(expected)
        for key, value in (("target", "ubuntu-26.04"), ("nonce", "../../host"), ("frame_sha256", "0" * 64)):
            altered = copy.deepcopy(expected)
            altered[key] = value
            with self.subTest(key=key), self.assertRaises(common.PackagingError):
                vm.verify_contract(altered)

    def test_missing_software_gate_cannot_be_promoted_by_a_vm_report(self):
        with self.assertRaisesRegex(common.PackagingError, "software gate"):
            vm.require_software({"validation": {"software": {}}})

    def test_unsupported_cli_target_fails_before_any_runtime_or_file_operation(self):
        with patch.object(sys, "argv", ["vm.py", "--target", "debian-12", "--prepare-only"]), \
                patch.object(vm, "require_build_user"), patch.object(vm, "prepare_runtime") as prepared, \
                self.assertRaises(common.PackagingError):
            vm.main()
        prepared.assert_not_called()


class ReportFrameTests(unittest.TestCase):
    def decode(self, expected, data):
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.log"
            serial.write_bytes(data)
            return vm.decode_report(serial, expected)

    def test_readiness_is_honestly_not_a_native_pass(self):
        expected = contract()
        value = report(expected)
        self.assertEqual(self.decode(expected, b"kernel boot\n" + frame(expected, value))["native_smoke"], "not-run")
        value["native_smoke"] = "passed"
        with self.assertRaisesRegex(common.PackagingError, "cannot certify"):
            self.decode(expected, frame(expected, value))

    def test_complete_bound_native_fixture_is_parsed_without_claiming_a_real_run(self):
        expected = contract("native")
        value = report(expected)
        self.assertEqual(self.decode(expected, frame(expected, value))["steps"], list(vm.NATIVE_STEPS))

    def test_missing_duplicate_or_altered_frame_is_rejected(self):
        expected = contract()
        valid = frame(expected, report(expected))
        for invalid in (b"", valid + valid, valid.replace(b"PHCI_VM_END_V1 ", b"PHCI_VM_END_V1 0"), valid.replace(expected["frame_sha256"].encode(), b"0" * 64)):
            with self.subTest(size=len(invalid)), self.assertRaises(common.PackagingError):
                self.decode(expected, invalid)

    def test_report_for_another_manifest_job_guest_or_user_is_rejected(self):
        expected = contract("native")
        for key, value in (("nonce", "3" * 32), ("manifest_sha256", "4" * 64), ("executor", "host")):
            altered = report(expected)
            altered[key] = value
            with self.subTest(key=key), self.assertRaises(common.PackagingError):
                self.decode(expected, frame(expected, altered))
        altered = report(expected)
        altered["guest"]["os_release"]["ID"] = "pop"
        with self.assertRaises(common.PackagingError):
            self.decode(expected, frame(expected, altered))
        altered = report(expected)
        altered["guest"]["user"]["uid"] = 0
        with self.assertRaises(common.PackagingError):
            self.decode(expected, frame(expected, altered))

    def test_partial_lifecycle_missing_unit_evidence_or_failed_agent_never_passes(self):
        expected = contract("native")
        for mutation in ("steps", "commands", "agent"):
            value = report(expected)
            if mutation == "steps":
                value["steps"].pop()
            elif mutation == "commands":
                del value["observations"][0]["commands"]["backend"]
            else:
                value["observations"][4]["commands"]["agent"]["returncode"] = 3
            with self.subTest(mutation=mutation), self.assertRaises(common.PackagingError):
                self.decode(expected, frame(expected, value))

    def test_native_report_must_match_command_status_hook_transaction_and_ready_semantics(self):
        expected = contract("native")
        for mutation in ("package_rc", "package_version", "backend_missing", "foreign_argv", "hook_inactive", "transaction_rc", "missing_remove_proof", "ready_false", "error"):
            value = report(expected)
            if mutation == "package_rc":
                value["observations"][0]["commands"]["package"]["returncode"] = 1
            elif mutation == "package_version":
                value["observations"][0]["commands"]["package"]["stdout"] = "ii  9.9.9\n"
            elif mutation == "backend_missing":
                value["observations"][0]["commands"]["backend"]["stdout"] = "LoadState=not-found\nActiveState=inactive\n"
            elif mutation == "foreign_argv":
                value["observations"][0]["commands"]["backend"]["argv"] = ["true"]
            elif mutation == "hook_inactive":
                value["observations"][1]["commands"]["backend"]["stdout"] = "MainPID=0\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nUnitFileState=enabled\n"
            elif mutation == "transaction_rc":
                value["observations"][0]["transactions"][0]["returncode"] = 1
            elif mutation == "missing_remove_proof":
                del value["observations"][11]["transactions"][0]["after_remove"]
            elif mutation == "ready_false":
                value["ready"] = False
            else:
                value["error"] = "guest operation failed"
            with self.subTest(mutation=mutation), self.assertRaises(common.PackagingError):
                self.decode(expected, frame(expected, value))


class ManifestPromotionTests(unittest.TestCase):
    def test_initial_manifest_writer_cannot_rehash_an_existing_verified_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, _ = bundle_fixture(root)
            (root / "app.deb").write_bytes(b"different package after verification")
            manifest["validation"]["native_smoke"] = "passed"
            with self.assertRaises(common.PackagingError):
                build.write_manifest(root, manifest)

    def test_already_promoted_vm_input_is_rejected_before_any_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, expected = bundle_fixture(root, promoted=True)
            with patch.object(sys, "argv", ["vm.py", "--bundle", directory, "--manifest-sha256", expected]), \
                    patch.object(vm, "require_build_user"), \
                    patch.object(vm, "prepare_runtime", side_effect=common.PackagingError("runtime reached")) as prepared, \
                    self.assertRaisesRegex(common.PackagingError, "already"):
                vm.main()
            prepared.assert_not_called()

    @unittest.skipUnless(sys.platform == "linux", "native atomic manifest CAS is Linux-only")
    def test_promotion_changes_validation_only_and_later_payload_change_is_not_recertified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, expected = bundle_fixture(root)
            promoted, new_sha = build.update_validation(root, expected, {"native_smoke": "passed", "native_smoke_evidence": {"input_manifest_sha256": expected}})
            self.assertEqual(promoted["files"], original["files"])
            self.assertEqual(promoted["sources"], original["sources"])
            self.assertEqual({key: value for key, value in promoted.items() if key != "validation"},
                             {key: value for key, value in original.items() if key != "validation"})
            self.assertEqual(common.verify_bundle(root, new_sha)[0], promoted)
            self.assertEqual(promoted["validation"]["native_smoke_evidence"]["input_manifest_sha256"], expected)
            (root / "source.tar.gz").write_bytes(b"changed source after promotion")
            with self.assertRaises(common.PackagingError):
                common.verify_bundle(root, new_sha)
            self.assertEqual(common.json_file(root / "manifest.json")["files"], original["files"])

    @unittest.skipUnless(sys.platform == "linux", "native atomic manifest CAS is Linux-only")
    def test_payload_swap_after_verification_before_promotion_invalidates_without_rehash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, expected = bundle_fixture(root)
            commit = build._cas_manifest
            calls = []
            def swapped(bundle, sha, manifest):
                calls.append(sha)
                if len(calls) == 1:
                    (root / "app.deb").write_bytes(b"concurrent different package")
                return commit(bundle, sha, manifest)
            with patch.object(build, "_cas_manifest", side_effect=swapped), self.assertRaises(common.PackagingError):
                build.update_validation(root, expected, {"native_smoke": "passed", "native_smoke_evidence": {"input_manifest_sha256": expected}})
            persisted = common.json_file(root / "manifest.json")
            self.assertEqual(persisted["files"], original["files"])
            self.assertEqual(persisted["sources"], original["sources"])
            self.assertEqual(persisted["validation"]["native_smoke"], "not-run")
            self.assertNotIn("native_smoke_evidence", persisted["validation"])
            with self.assertRaises(common.PackagingError):
                common.verify_bundle(root)

    @unittest.skipUnless(sys.platform == "linux", "native atomic manifest CAS is Linux-only")
    def test_manifest_writer_race_is_rejected_by_atomic_compare_exchange(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, expected = bundle_fixture(root)
            concurrent = copy.deepcopy(original)
            concurrent["validation"]["concurrent_note"] = "independent writer"
            commit = build._cas_manifest
            def replaced(bundle, sha, manifest):
                common.write_json(root / "manifest.json", concurrent)
                (root / "manifest.sha256").write_text(common.digest(common.regular_bytes(root / "manifest.json")) + "  manifest.json\n")
                return commit(bundle, sha, manifest)
            with patch.object(build, "_cas_manifest", side_effect=replaced), self.assertRaisesRegex(common.PackagingError, "atomic.*CAS"):
                build.update_validation(root, expected, {"native_smoke": "passed", "native_smoke_evidence": {"input_manifest_sha256": expected}})
            self.assertEqual(common.verify_bundle(root)[0], concurrent)
            self.assertEqual(concurrent["files"], original["files"])


class RuntimeIsolationTests(unittest.TestCase):
    def test_qemu_arguments_are_closed_tcg_only_without_host_devices_or_forwarding(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            for name in ("overlay.qcow2", "vars.fd", "seed.iso", "input.iso"):
                (work / name).write_bytes(b"fixture")
            args = vm.qemu_command(work, "1" * 32)
            vm.validate_qemu_args(args, work, "1" * 32)
            self.assertIn("tcg,thread=multi", args)
            for forbidden in ("-enable-kvm", "/dev/kvm", "-virtfs", "-fsdev", "-daemonize", "-audio", "hostfwd", "vfio"):
                self.assertFalse(any(forbidden in value for value in args))
            for extra in (["-enable-kvm"], ["-netdev", "user,hostfwd=tcp::2222-:22"], ["-device", "vfio-pci,host=00:00.0"]):
                with self.subTest(extra=extra), self.assertRaises(common.PackagingError):
                    vm.validate_qemu_args(args + extra, work, "1" * 32)
        with self.assertRaisesRegex(common.PackagingError, "inject"):
            vm.qemu_command(Path("/work,host_device=/dev/kvm"), "1" * 32)

    def test_docker_has_only_exact_readonly_source_bundle_and_owned_job_mounts(self):
        args = vm.docker_command(["docker"], "fixture", Path("/project/cache"), Path("/project/job"), Path("/project/bundle"), "owned")
        self.assertIn("--read-only", args)
        self.assertEqual(args[args.index("--user") + 1], "1000:1000")
        self.assertEqual(args[args.index("--cap-drop") + 1], "ALL")
        self.assertNotIn("--privileged", args)
        self.assertNotIn("--cap-add", args)
        self.assertNotIn("--device", args)
        for value in args:
            self.assertNotIn("docker.sock", value)
            self.assertNotIn("/dev/input", value)
            self.assertNotIn("/dev/snd", value)
            self.assertNotIn("hostfwd", value)

    def test_failed_container_invocation_force_removes_only_its_generated_name(self):
        commands = []
        def execute(args, **kwargs):
            commands.append(args)
            if args[1] == "run":
                raise common.PackagingError("fixture failure")
            return SimpleNamespace(returncode=0)
        with patch.object(vm, "run", side_effect=execute), self.assertRaisesRegex(common.PackagingError, "fixture failure"):
            vm.run_container(["docker"], "fixture", Path("/cache"))
        name = commands[0][commands[0].index("--name") + 1]
        self.assertRegex(name, r"^peripheral-hub-vm-[a-f0-9]{32}$")
        self.assertEqual(commands[1], ["docker", "rm", "--force", name])

    def test_qemu_monitor_failure_terminates_and_reaps_only_the_owned_process_group(self):
        process = MagicMock(pid=123456)
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 10), 0]
        selector = MagicMock()
        selector.get_map.return_value = {1: "fixture"}
        selector.select.side_effect = OSError("fixture reader failure")
        with tempfile.TemporaryDirectory() as directory, patch.object(vm.subprocess, "Popen", return_value=process), \
                patch.object(vm.selectors, "DefaultSelector") as factory, patch.object(vm.os, "killpg", create=True) as killed, \
                patch.object(vm.signal, "SIGTERM", 15), patch.object(vm.signal, "SIGKILL", 9, create=True):
            factory.return_value.__enter__.return_value = selector
            with self.assertRaisesRegex(OSError, "reader failure"):
                vm.monitor_qemu(["fixture"], Path(directory) / "serial.log", 120)
        self.assertEqual([call.args for call in killed.call_args_list], [(123456, 15), (123456, 9)])
        process.stdout.close.assert_called_once()

    def test_environment_marker_does_not_authorize_guest_operations_in_a_container(self):
        with patch.dict(os.environ, {"PH_CI_VM": "1"}), patch.object(vm_guest.sys, "platform", "linux"), \
                patch.object(vm_guest.platform, "machine", return_value="x86_64"), \
                patch.object(vm_guest.os, "geteuid", return_value=0, create=True), \
                patch.object(Path, "exists", return_value=True), self.assertRaisesRegex(common.PackagingError, "refuses OCI"):
            vm_guest.require_guest(contract())


if __name__ == "__main__":
    unittest.main()
