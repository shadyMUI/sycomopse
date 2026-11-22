import base64
import io
import json
import subprocess
import sys
import tempfile
import unittest
import zlib
from contextlib import ExitStack, redirect_stderr, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
FIXTURES = PACKAGING / "tests/fixtures"
sys.path.insert(0, str(PACKAGING))
import common
import install
import native
import system


def key_data(name):
    return common.regular_bytes(FIXTURES / (name + ".asc"), 128 * 1024)


def key_payload(name):
    lines = key_data(name).decode("ascii").splitlines()
    return base64.b64decode("".join(line for line in lines if line and not line.startswith(("-", "=")) and ":" not in line), validate=True)


def armor(payload):
    encoded = base64.b64encode(payload)
    return b"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n" + encoded + b"\n-----END PGP PUBLIC KEY BLOCK-----\n"


def packet(tag, body):
    return bytes([0xc0 | tag, 255]) + len(body).to_bytes(4, "big") + body


def noble_build():
    return {
        "id": "ubuntu-24.04", "os_release": {"ID": "ubuntu", "VERSION_ID": "24.04", "VERSION_CODENAME": "noble"},
        "image": "docker.io/library/ubuntu:24.04", "image_digest": "sha256:" + "a" * 64,
    }


def pop_fixture():
    return common.json_file(FIXTURES / "pop24.json")


def os_release_fixture(root, text):
    source, link = root / "os-release-real", root / "os-release"
    source.write_text(text, encoding="ascii")
    link.symlink_to(source)
    original_resolve = Path.resolve

    def resolve(path, *args, **kwargs):
        return original_resolve(link if str(path) == "/etc/os-release" else path, *args, **kwargs)

    return source, link, patch.object(Path, "resolve", resolve)


class RepositoryKeyTests(unittest.TestCase):
    def test_complete_locked_keys_and_real_armor_envelopes(self):
        pins = common.load_targets()["repository_keys"]
        for name, pin in (("obs-hardware", "obs_hardware"), ("openrazer-ppa", "openrazer_ppa")):
            original = key_data(name)
            variants = (original, original.replace(b"\n", b"\r\n"), b" \t\n" + original + b"\n\t ", armor(key_payload(name)))
            for variant in variants:
                with self.subTest(key=name, envelope=variant[:80]):
                    self.assertEqual(common.pgp_fingerprint(variant), pins[pin])

    def test_pinned_first_key_followed_by_different_primary_is_rejected(self):
        with self.assertRaises(common.PackagingError):
            common.pgp_fingerprint(armor(key_payload("obs-hardware") + key_payload("openrazer-ppa")))

    def test_duplicate_primary_is_rejected_even_when_both_match_the_pin(self):
        payload = key_payload("obs-hardware")
        with self.assertRaises(common.PackagingError):
            common.pgp_fingerprint(armor(payload + payload))

    def test_multiple_armor_blocks_are_rejected(self):
        for extra in (key_data("obs-hardware"), key_data("openrazer-ppa")):
            with self.subTest(extra=extra[:80]), self.assertRaises(common.PackagingError):
                common.pgp_fingerprint(key_data("obs-hardware") + extra)

    def test_compressed_or_special_key_bearing_packets_are_rejected(self):
        payload = key_payload("obs-hardware")
        other = key_payload("openrazer-ppa")
        for tag in (5, 7, 8, 10, 11, 12, 60, 61, 63):
            body = b"\x02" + zlib.compress(other) if tag == 8 else other
            for candidate in (payload + packet(tag, body), packet(tag, body) + payload):
                with self.subTest(tag=tag, prefix=candidate[:8]), self.assertRaises(common.PackagingError):
                    common.pgp_fingerprint(armor(candidate))

    def test_trailing_packet_and_length_truncations_are_rejected(self):
        payload = key_payload("obs-hardware")
        for trailing in (b"\x00", b"\xb4", b"\xb5\x00", b"\xb6\x00\x00\x00", b"\xc2", b"\xc2\xc0", b"\xc2\xff\x00\x00\x00", b"\xc2\x05\x04", b"\xc2\xe0", b"\x97"):
            with self.subTest(trailing=trailing), self.assertRaises(common.PackagingError):
                common.pgp_fingerprint(armor(payload + trailing))

    def test_armor_trailing_data_headers_checksums_and_non_ascii_are_rejected(self):
        original = key_data("obs-hardware")
        for candidate in (b"ignored\n" + original, original + b"ignored\n", original + b"\x00", original.replace(b"=Pbfs", b"=AAAA"), original.replace(b"=Pbfs", b"=Pbfs\n=Pbfs"), original.replace(b"=Pbfs", b"Comment: hidden payload"), original + b"\xff"):
            with self.subTest(candidate=candidate[-80:]), self.assertRaises(common.PackagingError):
                common.pgp_fingerprint(candidate)

    def test_old_and_new_definite_packet_lengths_preserve_the_primary(self):
        body = key_payload("obs-hardware")[3:272]
        self.assertEqual(len(body), 269)
        packets = (
            b"\x99" + len(body).to_bytes(2, "big") + body,
            b"\x9a" + len(body).to_bytes(4, "big") + body,
            b"\xc6\xc0\x4d" + body,
            packet(6, body),
        )
        for primary in packets:
            with self.subTest(header=primary[:6]):
                self.assertEqual(common.pgp_fingerprint(armor(primary)), common.load_targets()["repository_keys"]["obs_hardware"])

    def test_single_primary_retains_subkeys_user_ids_and_signatures(self):
        # The extra public-subkey packet tests framing, not cryptographic binding.
        # Only the package manager can authorize it via a primary-key signature.
        payload = key_payload("obs-hardware") + packet(14, key_payload("openrazer-ppa")[3:528])
        data = armor(payload)
        target = next(item for item in common.load_targets()["targets"] if item["id"] == "debian-12")
        with patch.object(system, "owned_file") as owned, patch.object(system, "run"):
            system.repository_setup(target, target["repository"], data, {})
        self.assertEqual(owned.call_args_list[0].args[1], data)
        self.assertEqual(common.pgp_fingerprint(data), common.load_targets()["repository_keys"]["obs_hardware"])

    def test_repository_import_preserves_all_official_key_bytes_for_apt_and_rpm(self):
        targets = {item["id"]: item for item in common.load_targets()["targets"]}
        for target_id, key in (("ubuntu-24.04", "openrazer-ppa"), ("fedora-44", "obs-hardware")):
            target, data = targets[target_id], key_data(key)
            with self.subTest(target=target_id), patch.object(system, "owned_file") as owned, patch.object(system, "run") as runner:
                system.repository_setup(target, target["repository"], data, {})
            self.assertEqual(owned.call_args_list[0].args[1], data)
            if target["format"] == "rpm":
                self.assertTrue(any(call.args[0][:2] == ["rpm", "--import"] for call in runner.call_args_list))

    def test_pin_bypass_is_rejected_before_any_repository_mutation(self):
        target = next(item for item in common.load_targets()["targets"] if item["id"] == "debian-12")
        for data in (armor(key_payload("obs-hardware") + key_payload("openrazer-ppa")), key_data("obs-hardware") + key_data("openrazer-ppa"), armor(key_payload("obs-hardware") + packet(8, b"\x02" + zlib.compress(key_payload("openrazer-ppa"))))):
            with self.subTest(data=data[:80]), patch.object(system, "owned_file") as owned, patch.object(system, "run") as runner:
                with self.assertRaises(common.PackagingError):
                    system.repository_setup(target, target["repository"], data, {})
            owned.assert_not_called()
            runner.assert_not_called()

    def test_regular_key_files_still_reject_symlinks_and_hardlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, alias = root / "key.asc", root / "alias.asc"
            source.write_bytes(key_data("obs-hardware"))
            alias.symlink_to(source)
            with self.assertRaises(common.PackagingError):
                common.regular_bytes(alias, 128 * 1024)
            alias.unlink()
            common.os.link(source, alias)
            with self.assertRaises(common.PackagingError):
                common.regular_bytes(source, 128 * 1024)


class PopCompatibilityTests(unittest.TestCase):
    def test_real_pop_release_maps_only_to_the_ubuntu_noble_build_target(self):
        release = common.parse_os_release(pop_fixture()["os_release"])
        target = common.target_for(release)
        self.assertEqual((target["id"], target["os_id"], target["image"]), ("ubuntu-24.04", "ubuntu", "docker.io/library/ubuntu:24.04"))
        self.assertEqual(release["ID"], "pop")
        self.assertEqual(len(common.load_targets()["targets"]), 10)
        release.pop("ID_LIKE")
        release.pop("VERSION_CODENAME")
        self.assertEqual(common.target_for(release)["id"], "ubuntu-24.04")

    def test_missing_wrong_codename_future_and_other_derivatives_are_rejected(self):
        release = common.parse_os_release(pop_fixture()["os_release"])
        invalid = [dict(release, UBUNTU_CODENAME=""), dict(release, UBUNTU_CODENAME="jammy"), dict(release, VERSION_ID="26.04"), dict(release, VERSION_ID="22.04"), dict(release, ID="linuxmint"), dict(release, VERSION_CODENAME="jammy")]
        missing = dict(release)
        missing.pop("UBUNTU_CODENAME")
        invalid.append(missing)
        for candidate in invalid:
            with self.subTest(release=candidate), self.assertRaises(common.PackagingError):
                common.target_for(candidate)

    def test_pop_is_compatible_host_not_native_build_provenance(self):
        release = common.parse_os_release(pop_fixture()["os_release"])
        with self.assertRaises(common.PackagingError):
            common.target_for(release, allow_compatible=False)
        self.assertEqual(common.target_for(noble_build()["os_release"], allow_compatible=False)["id"], "ubuntu-24.04")

    def test_preflight_accepts_real_ubuntu_build_and_rejects_fake_pop_build(self):
        fixture = pop_fixture()
        target = next(item for item in common.load_targets()["targets"] if item["id"] == "ubuntu-24.04")
        manifest = {"target": noble_build(), "package": {"format": "deb"}, "validation": {"native_smoke": "passed"}}
        install.preflight(target, fixture["facts"], manifest)
        manifest["target"]["os_release"] = common.parse_os_release(fixture["os_release"])
        with self.assertRaises(common.PackagingError):
            install.preflight(target, fixture["facts"], manifest)

    def test_build_target_rejects_mismatched_real_release_or_image(self):
        for changes in ({"os_release": {"ID": "ubuntu", "VERSION_ID": "22.04", "VERSION_CODENAME": "jammy"}}, {"image": "docker.io/library/ubuntu:22.04"}, {"image_digest": "not-a-digest"}):
            with self.subTest(changes=changes), self.assertRaises(common.PackagingError):
                common.build_target(dict(noble_build(), **changes), "deb")

    def test_pop_dry_run_keeps_real_fixture_and_does_not_elevate(self):
        result = subprocess.run([sys.executable, "-B", str(PACKAGING / "install.py"), "--dry-run", "--json", "--fixture", str(FIXTURES / "pop24.json")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        plan = json.loads(result.stdout)
        self.assertEqual(plan["target"], "ubuntu-24.04")
        self.assertEqual(plan["host_os_release"]["ID"], "pop")
        self.assertTrue(plan["compatible_host"])
        self.assertNotIn("sudo", result.stdout)


class InstalledPackageTests(unittest.TestCase):
    def test_installed_and_uninstalled_are_distinct_from_query_errors(self):
        for manager, stdout in (("apt", "installed 0.1.0-1"), ("pacman", "peripheral-hub 0.1.0-1"), ("dnf", "0.1.0-1"), ("zypper", "0.1.0-1")):
            with self.subTest(manager=manager), patch.object(install, "run", return_value=SimpleNamespace(returncode=0, stdout=stdout, stderr="")):
                self.assertEqual(install.installed(manager, common.APP), "0.1.0-1")
        for manager, stdout, stderr in (("apt", "", "dpkg-query: no packages found matching peripheral-hub"), ("pacman", "", "error: package 'peripheral-hub' was not found"), ("dnf", "package peripheral-hub is not installed", ""), ("zypper", "package peripheral-hub is not installed", "")):
            with self.subTest(manager=manager), patch.object(install, "run", return_value=SimpleNamespace(returncode=1, stdout=stdout, stderr=stderr)):
                self.assertIsNone(install.installed(manager, common.APP))
        for manager in ("apt", "pacman", "dnf", "zypper"):
            with self.subTest(manager=manager), patch.object(install, "run", return_value=SimpleNamespace(returncode=1, stdout="", stderr="cannot open package database: Permission denied")):
                with self.assertRaises(common.PackagingError):
                    install.installed(manager, common.APP)

    def test_partial_deb_configuration_is_present_but_not_ready(self):
        with patch.object(install, "run", return_value=SimpleNamespace(returncode=0, stdout="half-configured 0.1.0-1", stderr="")):
            self.assertIsNone(install.installed("apt", common.APP))
            self.assertEqual(install.installed("apt", common.APP, details=True), {"state": "half-configured", "version": "0.1.0-1"})


class NativeHostValidationTests(unittest.TestCase):
    def invoke(self, release, provenance=None, action="configure", package_format="deb"):
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            metadata = Path(directory) / "build-target.json"
            if provenance is not False:
                common.write_json(metadata, dict(provenance or noble_build(), schema=1))
            source, link, resolver = os_release_fixture(Path(directory), release)
            stack.enter_context(resolver)
            stack.enter_context(patch.object(native, "BUILD_TARGET", metadata, create=True))
            stack.enter_context(patch.object(native, "STATUS", Path(directory) / "native-status.json"))
            failure_loader = native.native_failure
            stack.enter_context(patch.object(native, "native_failure", side_effect=lambda path: failure_loader(path, owner=common.os.getuid())))
            checkpoint = native.configure_checkpoint
            stack.enter_context(patch.object(native, "configure_checkpoint", side_effect=lambda *args, **kwargs: checkpoint(*args, owner=common.os.getuid(), **kwargs)))
            stack.enter_context(patch.object(native, "regular_bytes", side_effect=lambda path, maximum: common.regular_bytes(link if str(path) == "/etc/os-release" else path, maximum), create=True))
            stack.enter_context(patch.object(native.os, "geteuid", return_value=0))
            stack.enter_context(patch.object(sys, "argv", [str(PACKAGING / "native.py"), action, "--format", package_format]))
            configured = stack.enter_context(patch.object(native, "configure", return_value={"pending": [], "imports_verified": True}))
            removed = stack.enter_context(patch.object(native, "remove"))
            stack.enter_context(redirect_stderr(io.StringIO()))
            code = native.main()
            return code, configured, removed

    def test_native_configure_accepts_explicit_pop_host_for_real_noble_payload(self):
        code, configured, removed = self.invoke(pop_fixture()["os_release"])
        self.assertEqual(code, 0)
        configured.assert_called_once()
        removed.assert_not_called()
        self.assertEqual(configured.call_args.kwargs["host"]["os_release"]["ID"], "pop")
        self.assertEqual(configured.call_args.kwargs["host"]["build_target"]["os_release"]["ID"], "ubuntu")

    def test_native_configure_rejects_bad_host_and_payload_before_services(self):
        real = pop_fixture()["os_release"]
        candidates = (
            (real.replace("UBUNTU_CODENAME=noble\n", ""), None, "deb"),
            (real.replace("UBUNTU_CODENAME=noble", "UBUNTU_CODENAME=jammy"), None, "deb"),
            (real.replace('VERSION_ID="24.04"', 'VERSION_ID="26.04"'), None, "deb"),
            (real, dict(noble_build(), os_release=common.parse_os_release(real)), "deb"),
            (real, False, "deb"),
            (real, None, "rpm"),
        )
        for release, provenance, format in candidates:
            with self.subTest(release=release, provenance=provenance, format=format):
                code, configured, removed = self.invoke(release, provenance, package_format=format)
                self.assertEqual(code, 1)
                configured.assert_not_called()
                removed.assert_not_called()

    def test_remove_remains_available_after_an_unsupported_os_upgrade(self):
        code, configured, removed = self.invoke("ID=pop\nVERSION_ID=26.04\nUBUNTU_CODENAME=resolute\n", provenance=False, action="remove")
        self.assertEqual(code, 0)
        configured.assert_not_called()
        removed.assert_called_once()


class InstallerStatusTests(unittest.TestCase):
    def observe(self, package="0.1.0-1", system_record=None, native_record=None, agent=True, engine=True, headset=None, private=False, cli_status=False, package_state="installed", query_error=False):
        target = next(item for item in common.load_targets()["targets"] if item["id"] == "ubuntu-24.04")
        headset = headset or {"configured": True, "device_count": 0, "user_access": "unknown", "reason": "No attached headsets; software verified"}
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            state, checkpoint, cli = root / "installer.json", root / "native-status.json", root / "headsetcontrol"
            if system_record is not None:
                common.write_json(state, dict(schema=1, app=common.APP, **system_record))
            if native_record is not None:
                common.write_json(checkpoint, dict(schema=1, format="deb", engine=common.ENGINE, **native_record))
            if private:
                cli.write_bytes(b"fixture CLI, never executed")
            stack.enter_context(patch.object(install, "STATE", state))
            stack.enter_context(patch.object(install, "NATIVE_STATE", checkpoint, create=True))
            stack.enter_context(patch.object(install, "NATIVE_FAILURE", root / "native-failure.json"))
            stack.enter_context(patch.object(install, "native_failure", side_effect=lambda path: common.native_failure(path, owner=common.os.getuid())))
            stack.enter_context(patch.object(install, "HEADSET", cli, create=True))
            stack.enter_context(patch.object(install, "installed", return_value={"version": package, "state": package_state} if package else None, side_effect=common.PackagingError("package database unreadable") if query_error else None))
            probe = stack.enter_context(patch.object(install, "probe_headset", return_value=headset))

            def service(argv, **kwargs):
                self.assertIn("is-active", argv)
                self.assertEqual(argv[0], "systemctl")
                active = agent if "--user" in argv else engine
                return SimpleNamespace(returncode=0 if active else 3, stdout="active" if active else "inactive")

            runner = stack.enter_context(patch.object(install, "run", side_effect=service))
            facts = dict(pop_fixture()["facts"], headsetcontrol=headset if not private else {"configured": False})
            if cli_status:
                output = io.StringIO()
                source, link, resolver = os_release_fixture(root, pop_fixture()["os_release"])
                stack.enter_context(resolver)
                stack.enter_context(patch.object(install.os, "geteuid", return_value=1000))
                stack.enter_context(patch.object(install, "regular_bytes", side_effect=lambda path, maximum: common.regular_bytes(link if str(path) == "/etc/os-release" else path, maximum)))
                stack.enter_context(patch.object(install, "facts_for", return_value=facts))
                stack.enter_context(patch.object(sys, "argv", [str(PACKAGING / "install.py"), "status", "--bundle", str(root)]))
                stack.enter_context(redirect_stdout(output))
                self.assertEqual(install.main(), 0)
                result = json.loads(output.getvalue())
            else:
                result = install.installation_status(target, facts)
            self.assertTrue(all("is-active" in call.args[0] for call in runner.call_args_list))
            if not package:
                probe.assert_not_called()
            return result

    def test_native_install_without_installer_json_can_be_running(self):
        result = self.observe(native_record={"status": "running", "pending": [], "running": True})
        self.assertEqual(result["status"], "running")
        self.assertTrue(result["package_installed"])
        self.assertEqual(result["package_version"], "0.1.0-1")
        self.assertEqual(result["native_record"]["status"], "running")

    def test_status_cli_observes_native_install_on_pop_without_elevation(self):
        result = self.observe(native_record={"status": "running", "pending": []}, private=True, cli_status=True)
        self.assertEqual(result["status"], "running")
        self.assertEqual(result["host_os_release"]["ID"], "pop")
        self.assertEqual(result["target"], "ubuntu-24.04")
        self.assertTrue(result["compatible_host"])

    def test_present_package_without_any_checkpoint_is_never_not_installed(self):
        self.assertEqual(self.observe()["status"], "running")
        for agent, engine in ((False, True), (True, False), (False, False)):
            with self.subTest(agent=agent, engine=engine):
                result = self.observe(agent=agent, engine=engine)
                self.assertEqual(result["status"], "pending")
                self.assertTrue(result["pending"])

    def test_partial_deb_is_pending_even_with_stale_running_checkpoint(self):
        result = self.observe(package_state="half-configured", native_record={"status": "running", "pending": []})
        self.assertEqual(result["status"], "pending")
        self.assertTrue(result["package_installed"])
        self.assertEqual(result["package_state"], "half-configured")

    def test_package_query_error_is_unknown_not_not_installed(self):
        result = self.observe(query_error=True, native_record={"status": "running", "pending": []})
        self.assertEqual(result["status"], "unknown")
        self.assertIsNone(result["package_installed"])
        self.assertTrue(result["pending"])

    def test_stale_removed_record_does_not_hide_a_native_reinstallation(self):
        result = self.observe(system_record={"status": "removed", "pending": []}, native_record={"status": "running", "pending": []})
        self.assertEqual(result["status"], "running")

    def test_absent_package_is_not_running_based_only_on_stale_records(self):
        self.assertEqual(self.observe(package=None, native_record={"status": "running", "pending": []})["status"], "pending")
        self.assertEqual(self.observe(package=None, native_record={"status": "removed", "pending": []}, engine=False)["status"], "removed")

    def test_stale_native_pending_boot_is_superseded_by_live_services(self):
        result = self.observe(native_record={"status": "pending-boot", "pending": ["Service configured offline; starts when systemd boots"]})
        self.assertEqual(result["status"], "running")

    def test_native_disabled_checkpoint_remains_diagnostic_when_service_is_inactive(self):
        reason = "Private backend disabled by administrator or preset; state preserved"
        result = self.observe(engine=False, native_record={"status": "disabled", "pending": [reason]})
        self.assertEqual(result["status"], "pending")
        self.assertIn(reason, result["pending"])

    def test_legacy_replug_warning_is_informational_after_a_healthy_user_probe(self):
        reason = "headsetcontrol: reconnect existing headsets if new uaccess rules have not taken effect"
        result = self.observe(system_record={"status": "pending", "pending": [reason]})
        self.assertEqual(result["status"], "running")
        self.assertEqual(result["pending"], [])
        self.assertIn(reason, result["notices"])

    def test_real_system_pending_and_failed_transaction_are_not_hidden(self):
        reason = "openrazer: Secure Boot signing/MOK enrollment pending"
        self.assertIn(reason, self.observe(system_record={"status": "pending", "pending": [reason]})["pending"])
        self.assertIn(self.observe(system_record={"status": "failed", "pending": [], "error": "DKMS failed"})["status"], ("pending", "failed"))

    def test_private_cli_without_attached_headsets_is_not_perpetually_pending(self):
        result = self.observe(private=True)
        self.assertEqual(result["status"], "running")
        self.assertEqual(result["headsetcontrol"]["user_access"], "unknown")

    def test_real_headset_failure_is_pending_with_the_actual_api_and_unknown_access(self):
        result = self.observe(private=True, headset={"configured": False, "reason": "Permission denied", "user_access": "unknown"})
        self.assertEqual(result["status"], "pending")
        self.assertTrue(any("API 1.6" in reason for reason in result["pending"]))
        self.assertFalse(any("1.8/1.9" in reason for reason in result["pending"]))
        self.assertEqual(result["headsetcontrol"]["user_access"], "unknown")

    def test_malformed_checkpoint_notices_are_rejected_not_reported_as_ready(self):
        for notices in ("not a list", [None], [{}]):
            with self.subTest(notices=notices), self.assertRaises(common.PackagingError):
                self.observe(system_record={"status": "running", "pending": [], "notices": notices})


class HeadsetUserProbeTests(unittest.TestCase):
    def probe(self, devices, uid=1000, code=0, stderr=b""):
        output = json.dumps({"name": "HeadsetControl", "api_version": "1.6", "device_count": len(devices), "devices": devices}).encode()

        def command(argv, **kwargs):
            kwargs["stdout"].write(output)
            kwargs["stderr"].write(stderr)
            return SimpleNamespace(returncode=code)

        with patch.object(common.os, "geteuid", return_value=uid), patch.object(common.subprocess, "run", side_effect=command):
            return common.probe_headset("/fixture/headsetcontrol")

    def device(self, **changes):
        return dict(common.json_file(FIXTURES / "headsetcontrol-4.2.json")["devices"][0], **changes)

    def test_successful_battery_read_is_user_scoped_not_a_root_permission_claim(self):
        for uid, access in ((1000, "verified"), (0, "unknown")):
            with self.subTest(uid=uid):
                result = self.probe([self.device()], uid=uid)
                self.assertTrue(result["configured"])
                self.assertEqual(result["user_access"], access)
                self.assertEqual(result["probe_uid"], uid)

    def test_no_attached_headsets_verify_software_but_leave_permissions_unknown(self):
        result = self.probe([], code=1)
        self.assertTrue(result["configured"])
        self.assertEqual(result["device_count"], 0)
        self.assertEqual(result["user_access"], "unknown")

    def test_inventory_without_a_sample_is_not_permission_evidence(self):
        result = self.probe([self.device(battery={"status": "BATTERY_UNAVAILABLE", "level": -1})])
        self.assertTrue(result["configured"])
        self.assertEqual(result["user_access"], "unknown")

    def test_json_device_errors_and_stderr_are_real_failures_not_success(self):
        for device, stderr in ((self.device(status="failure", errors={"HID": "Permission denied"}), b""), (self.device(battery={"status": "BATTERY_ERROR", "level": -1}), b""), (self.device(), b"Permission denied")):
            with self.subTest(device=device, stderr=stderr):
                result = self.probe([device], stderr=stderr)
                self.assertFalse(result["configured"])
                self.assertEqual(result["user_access"], "unknown")


class UserTransactionTests(unittest.TestCase):
    def invoke(self, native_outcome=None, system_status="running", root_rc=0):
        data = key_data("openrazer-ppa")
        target = next(item for item in common.load_targets()["targets"] if item["id"] == "ubuntu-24.04")
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            state, cli = root / "installer.json", root / "headsetcontrol"
            common.write_json(state, {"schema": 1, "app": common.APP, "status": system_status, "package_installed": True, "pending": [], "notices": [common.HEADSET_REPLUG_NOTICE]})
            failure = root / "native-failure.json"
            if native_outcome:
                common.write_json(failure, native_outcome)
            (root / "manifest.json").write_bytes(b"{}")
            cli.write_bytes(b"fixture CLI, never executed")
            manifest = {"target": noble_build(), "app": {"version": "0.1.0"}, "package": {"file": "app.deb", "format": "deb"}}
            manifest["validation"] = {"native_smoke": "passed"}
            plan = {"target": target["id"], "action": "repair", "pending": [], "packages": [], "commands": [], "repository": target["repository"]}
            source, link, resolver = os_release_fixture(root, "ID=ubuntu\nVERSION_ID=24.04\nVERSION_CODENAME=noble\n")
            stack.enter_context(resolver)
            stack.enter_context(patch.object(install, "STATE", state))
            stack.enter_context(patch.object(install, "NATIVE_FAILURE", root / "native-failure.json"))
            stack.enter_context(patch.object(install, "native_failure", side_effect=lambda path: common.native_failure(path, owner=common.os.getuid())))
            stack.enter_context(patch.object(install, "HEADSET", cli))
            stack.enter_context(patch.object(install.os, "geteuid", return_value=1000))
            stack.enter_context(patch.dict(install.os.environ, {"XDG_STATE_HOME": str(root / "user-state")}))
            stack.enter_context(patch.object(install, "regular_bytes", side_effect=lambda path, maximum: common.regular_bytes(link if str(path) == "/etc/os-release" else path, maximum)))
            stack.enter_context(patch.object(install, "facts_for", return_value=pop_fixture()["facts"]))
            stack.enter_context(patch.object(install, "verify_bundle", return_value=(manifest, "a" * 64)))
            stack.enter_context(patch.object(install, "system_plan", return_value=plan))
            user_operations = stack.enter_context(patch.object(install, "user_commands", return_value=[]))
            stack.enter_context(patch.object(install, "https_bytes", return_value=data))
            stack.enter_context(patch.object(install.shutil, "which", side_effect=lambda name: "/fixture/sudo" if name == "sudo" else None))
            probe = stack.enter_context(patch.object(install, "probe_headset", return_value={"configured": True, "device_count": 0, "user_access": "unknown"}))
            original_write = Path.write_bytes

            def changed_key(path, contents):
                return original_write(path, contents + b"\n" if path.name == "repository.asc" else contents)

            stack.enter_context(patch.object(Path, "write_bytes", changed_key))

            def authorized(argv, **kwargs):
                self.assertEqual(argv[:2], ["sudo", "--"])
                bootstrap = argv.index(install.ROOT_BOOTSTRAP)
                request = common.json_file(argv[bootstrap + 2])
                self.assertEqual(request["repository_key_sha256"], common.digest(data))
                self.assertNotEqual(common.digest(common.regular_bytes(argv[-1], 128 * 1024)), request["repository_key_sha256"])
                return SimpleNamespace(returncode=root_rc)

            runner = stack.enter_context(patch.object(install, "run", side_effect=authorized))
            stack.enter_context(patch.object(sys, "argv", [str(PACKAGING / "install.py"), "repair", "--bundle", str(root), "--yes", "--json", "--consent-repository"]))
            output = io.StringIO()
            stack.enter_context(redirect_stdout(output))
            code = install.main()
            runner.assert_called_once()
            summary = common.json_file(root / "user-state/peripheral-hub/installer-status.json")
            if native_outcome or root_rc or system_status == "failed":
                self.assertEqual(user_operations.call_count, 1, "Only planning, not user activation, is permitted after a failed native transaction")
                probe.assert_not_called()
            return code, summary

    def test_download_checksum_is_pinned_before_the_user_key_file_can_change(self):
        code, summary = self.invoke()
        self.assertEqual(code, 0)
        self.assertEqual(summary["status"], "running")
        self.assertEqual(summary["pending"], [])
        self.assertEqual(summary["headsetcontrol"]["user_access"], "unknown")

    def test_native_failed_post_with_zero_manager_exit_blocks_user_activation_and_ready_summary(self):
        proof = {"schema": 1, "app": common.APP, "engine": common.ENGINE, "format": "rpm", "status": "failed", "error": "RPM %post import failed"}
        code, summary = self.invoke(native_outcome=proof)
        self.assertEqual(code, 1)
        self.assertEqual(summary["status"], "failed")
        self.assertEqual(summary["native_failure"], proof)

    def test_installed_package_after_failed_system_transaction_does_not_start_user_phase(self):
        code, summary = self.invoke(system_status="failed", root_rc=1)
        self.assertEqual(code, 1)
        self.assertEqual(summary["status"], "failed")


class SystemPhaseTests(unittest.TestCase):
    def invoke(self, action="install", private=True, pending=(), old_state=None, repository_key=None, expected_key=None, app_version="0.1.0-1", target_id="ubuntu-24.04", native_outcome=None):
        fixture = pop_fixture()
        target = next(item for item in common.load_targets()["targets"] if item["id"] == target_id)
        release = common.json_file(FIXTURES / "opensuse16.json")["os_release"] if target_id == "opensuse-leap-16.0" else "ID=ubuntu\nVERSION_ID=24.04\nVERSION_CODENAME=noble\n"
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            state = root / "installer.json"
            if old_state:
                common.write_json(state, dict(schema=1, app=common.APP, **old_state))
            package = root / ("app." + target["format"])
            package.write_bytes(b"fixture native package")
            request_path, key_path = root / "request.json", root / "repository.asc"
            choices = dict.fromkeys(("audio", "openrazer", "audio_switch", "session_manager", "ratbag", "repository", "unsigned_dkms"), True)
            request = {"schema": 1, "uid": 1000, "action": action, "target": target_id, "choices": choices, "manifest_sha256": "a" * 64, "yes": True, "purge": False, "repository_key_sha256": expected_key}
            provenance = {"id": target_id, "os_release": common.parse_os_release(release), "image": target["image"], "image_digest": "sha256:" + "a" * 64}
            manifest = {"target": provenance, "package": {"file": package.name, "format": target["format"]}, "app": {"version": "0.1.0"}, "files": {package.name: {"size": len(package.read_bytes()), "sha256": common.digest(package.read_bytes())}}}
            release_source, release_link, resolver = os_release_fixture(root, release)
            stack.enter_context(resolver)

            def read(path, maximum=common.MAX_JSON, owner=None):
                if Path(path) == release_source or str(path) == "/etc/os-release":
                    return common.regular_bytes(release_link if str(path) == "/etc/os-release" else path, maximum, owner)
                if Path(path) == request_path:
                    return json.dumps(request).encode()
                if Path(path) == key_path:
                    self.assertEqual((maximum, owner), (128 * 1024, 1000))
                    return repository_key
                return common.regular_bytes(path, maximum, owner)

            original_is_file = Path.is_file
            original_temporary = tempfile.TemporaryDirectory
            stack.enter_context(patch.object(Path, "is_file", lambda path: private if str(path) == "/usr/lib/peripheral-hub/headsetcontrol" else original_is_file(path)))
            stack.enter_context(patch.object(system, "STATE", state))
            stack.enter_context(patch.object(system, "NATIVE_FAILURE", root / "native-failure.json"))
            stack.enter_context(patch.object(system, "native_failure", side_effect=lambda path: common.native_failure(path, owner=common.os.getuid())))
            reader = stack.enter_context(patch.object(system, "regular_bytes", side_effect=read))
            stack.enter_context(patch.object(system.os, "geteuid", return_value=0))
            stack.enter_context(patch.dict(system.os.environ, {"SUDO_UID": "1000"}))
            stack.enter_context(patch.object(system.pwd, "getpwuid", return_value=SimpleNamespace(pw_name="fixture-user")))
            stack.enter_context(patch.object(system, "verify_bundle", return_value=(manifest, "a" * 64)))
            stack.enter_context(patch.object(system, "facts_for", return_value=fixture["facts"]))
            stack.enter_context(patch.object(system, "check_repository_policy"))
            stack.enter_context(patch.object(system, "installed", return_value=app_version))
            spec = target["repository"] if repository_key is not None else None
            stack.enter_context(patch.object(system, "system_plan", return_value={"pending": list(pending), "packages": [], "repository": spec, "commands": []}))
            resolved = [{"name": "openrazer-daemon", "version": "3.3.0", "requirement": "openrazer-daemon"}] if spec else []
            stack.enter_context(patch.object(system, "resolve_packages", side_effect=[(resolved, []), ([], [])] if spec else [(resolved, [])]))
            setup = stack.enter_context(patch.object(system, "repository_setup"))
            stack.enter_context(patch.object(system, "native_install", side_effect=lambda *args, **kwargs: common.write_json(root / "native-failure.json", native_outcome) if native_outcome else None))
            units = stack.enter_context(patch.object(system, "system_unit"))
            stack.enter_context(patch.object(system, "configure_razer"))
            stack.enter_context(patch.object(system, "run", return_value=SimpleNamespace(returncode=0, stdout="", stderr="")))
            probe = stack.enter_context(patch.object(common, "probe_headset", side_effect=AssertionError("Root must not claim user HID permissions")))
            stack.enter_context(patch.object(system.tempfile, "TemporaryDirectory", side_effect=lambda **kwargs: original_temporary(prefix=kwargs.get("prefix", "fixture-"), dir=root)))
            argv = [str(PACKAGING / "system.py"), "--bundle", str(root), "--request", str(request_path)]
            if spec:
                argv += ["--key", str(key_path)]
            stack.enter_context(patch.object(sys, "argv", argv))
            stack.enter_context(redirect_stdout(io.StringIO()))
            stack.enter_context(redirect_stderr(io.StringIO()))
            code = system.main()
            probe.assert_not_called()
            if native_outcome:
                units.assert_not_called()
            if spec:
                self.assertEqual(sum(Path(call.args[0]) == key_path for call in reader.call_args_list), 1)
            return code, common.json_file(state), setup

    def test_success_and_healthy_repair_do_not_acquire_a_permanent_replug_pending(self):
        legacy = "headsetcontrol: reconnect existing headsets if new uaccess rules have not taken effect"
        for action, old in (("install", None), ("repair", {"status": "pending", "pending": [legacy]})):
            with self.subTest(action=action):
                code, record, _ = self.invoke(action=action, old_state=old)
                self.assertEqual(code, 0)
                self.assertEqual(record["status"], "running")
                self.assertEqual(record["pending"], [])
                self.assertTrue(record["notices"])
                self.assertEqual(record["headsetcontrol"]["user_access"], "unknown")

    def test_successful_repair_clears_a_previous_transaction_error(self):
        code, record, _ = self.invoke(action="repair", old_state={"status": "failed", "pending": [], "error": "old failure"})
        self.assertEqual(code, 0)
        self.assertEqual(record["status"], "running")
        self.assertNotIn("error", record)

    def test_missing_native_package_cannot_be_reported_as_running(self):
        code, record, _ = self.invoke(app_version=None)
        self.assertEqual(code, 1)
        self.assertEqual(record["status"], "failed")
        self.assertFalse(record["package_installed"])

    def test_rpm_zero_manager_exit_with_failed_post_is_still_a_failed_system_transaction(self):
        proof = {"schema": 1, "app": common.APP, "engine": common.ENGINE, "format": "rpm", "status": "failed", "error": "RPM %post import failed"}
        code, record, _ = self.invoke(target_id="opensuse-leap-16.0", native_outcome=proof)
        self.assertEqual(code, 1)
        self.assertEqual(record["status"], "failed")
        self.assertTrue(record["package_installed"])
        self.assertIn(proof["error"], record["error"])

    def test_uninstall_does_not_claim_removed_when_package_is_still_present(self):
        code, record, _ = self.invoke(action="uninstall")
        self.assertEqual(code, 1)
        self.assertEqual(record["status"], "failed")

    def test_no_private_cli_does_not_add_a_replug_warning(self):
        code, record, _ = self.invoke(private=False)
        self.assertEqual(code, 0)
        self.assertEqual(record["status"], "running")
        self.assertEqual(record.get("notices", []), [])

    def test_real_pending_is_preserved(self):
        reason = "openrazer: Secure Boot module-signing/MOK enrollment pending"
        code, record, _ = self.invoke(pending=[reason])
        self.assertEqual(code, 0)
        self.assertEqual(record["status"], "pending")
        self.assertEqual(record["pending"], [reason])

    def test_root_uses_the_exact_checksum_pinned_regular_key_bytes(self):
        data = key_data("openrazer-ppa")
        code, record, setup = self.invoke(repository_key=data, expected_key=common.digest(data))
        self.assertEqual(code, 0)
        self.assertEqual(record["status"], "running")
        self.assertEqual(setup.call_args.args[2], data)
        code, record, setup = self.invoke(repository_key=data, expected_key="b" * 64)
        self.assertEqual(code, 1)
        self.assertEqual(record["status"], "failed")
        setup.assert_not_called()


if __name__ == "__main__":
    unittest.main()
