import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import common
import native
from build import native_requirements


class Commands:
    def __init__(self, enabled="disabled", active=False, preset=True, policy=True, failures=()):
        self.enabled, self.active, self.preset, self.policy = enabled, active, preset, policy
        self.failures = set(failures)
        self.calls = []

    def __call__(self, argv, **kwargs):
        self.calls.append(argv)
        command = [part for part in argv if part != "--root=/"]
        stdout, code = "", 0
        if command[1] in self.failures:
            stdout, code = "simulated command failure\n", 1
        elif command[:2] == ["systemctl", "is-enabled"]:
            stdout, code = self.enabled + "\n", 0 if self.enabled == "enabled" else 1
        elif command[:2] == ["systemctl", "is-active"]:
            stdout, code = ("active\n", 0) if self.active else ("inactive\n", 3)
        elif command[:2] == ["deb-systemd-helper", "enable"]:
            self.enabled = "enabled"
        elif command[:2] == ["systemctl", "preset"]:
            self.enabled = "enabled" if self.preset else "disabled"
        elif command[0] == "deb-systemd-invoke":
            if self.policy:
                self.active = True
        elif command[:2] in (["systemctl", "start"], ["systemctl", "restart"]):
            self.active = True
        elif command[:2] == ["systemctl", "try-restart"]:
            self.active = self.active
        elif command[:2] == ["systemctl", "stop"]:
            self.active = False
        elif command[1:2] == ["disable"]:
            self.enabled = "disabled"
        if kwargs.get("check", True) and code:
            raise common.PackagingError("Simulated failure: " + " ".join(command))
        return SimpleNamespace(stdout=stdout, stderr="", returncode=code)


class NativeLifecycleTests(unittest.TestCase):
    def configure(self, format, commands, status, upgrade=False, live=True):
        return native.configure(format, upgrade, commands, status, live, lambda name: "/usr/bin/" + name)

    def remove(self, format, commands, status, live=True):
        return native.remove(format, commands, live, lambda name: "/usr/bin/" + name, status=status)

    def test_fresh_deb_enables_and_starts_with_debian_policy_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands()
            report = self.configure("deb", commands, Path(directory) / "state.json")
            self.assertEqual(report["status"], "running")
            self.assertIn(["deb-systemd-helper", "enable", common.ENGINE], commands.calls)
            self.assertIn(["deb-systemd-invoke", "start", common.ENGINE], commands.calls)
            self.assertFalse(any("--user" in command for command in commands.calls))
            self.assertFalse(any(command[0] in ("apt", "apt-get", "dnf", "pacman", "zypper", "sudo", "pkexec") for command in commands.calls))

    def test_fresh_rpm_and_arch_use_only_the_package_preset(self):
        for format in ("rpm", "arch"):
            with self.subTest(format=format), tempfile.TemporaryDirectory() as directory:
                commands = Commands()
                self.assertEqual(self.configure(format, commands, Path(directory) / "state.json")["status"], "running")
                self.assertIn(["systemctl", "preset", common.ENGINE], commands.calls)
                self.assertIn(["systemctl", "start", common.ENGINE], commands.calls)
                self.assertFalse(any("ratbagd.service" in command or "openrazer-daemon.service" in command for command in commands.calls))

    def test_administrator_mask_is_preserved_on_install_and_upgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands(enabled="masked")
            report = self.configure("deb", commands, Path(directory) / "state.json")
            self.assertEqual(report["status"], "masked")
            self.assertFalse(any(command[0] == "deb-systemd-helper" or "unmask" in command for command in commands.calls))

    def test_administrator_preset_can_disable_arch_and_rpm_startup(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands(preset=False)
            report = self.configure("arch", commands, Path(directory) / "state.json")
            self.assertEqual(report["status"], "disabled")
            self.assertFalse(any(command[:2] == ["systemctl", "start"] for command in commands.calls))

    def test_debian_policy_rc_d_denial_is_explicit_pending(self):
        with tempfile.TemporaryDirectory() as directory:
            report = self.configure("deb", Commands(policy=False), Path(directory) / "state.json")
            self.assertEqual(report["status"], "pending")
            self.assertFalse(report["running"])

    def test_upgrade_preserves_disabled_or_stopped_service(self):
        for format in ("deb", "rpm", "arch"):
            with self.subTest(format=format), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                common.write_json(status, {"schema": 1})
                commands = Commands(enabled="disabled")
                self.assertEqual(self.configure(format, commands, status, True)["status"], "disabled")
                self.assertFalse(any("preset" in command or command[0] == "deb-systemd-helper" for command in commands.calls))
                commands = Commands(enabled="enabled", active=False)
                self.assertEqual(self.configure(format, commands, status, True)["status"], "pending")
                self.assertFalse(commands.active)

    def test_running_upgrade_reloads_private_daemon_only(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            common.write_json(status, {"schema": 1})
            commands = Commands(enabled="enabled", active=True)
            self.assertEqual(self.configure("deb", commands, status, True)["status"], "running")
            self.assertIn(["deb-systemd-invoke", "restart", common.ENGINE], commands.calls)

    def test_upgrade_without_registry_never_enables_or_starts_an_admin_disabled_or_stopped_unit(self):
        for format in ("deb", "rpm", "arch"):
            for enabled in ("disabled", "enabled"):
                with self.subTest(format=format, enabled=enabled), tempfile.TemporaryDirectory() as directory:
                    commands = Commands(enabled=enabled)
                    report = self.configure(format, commands, Path(directory) / "state.json", upgrade=True)
                    self.assertEqual(report["enabled"], enabled == "enabled")
                    self.assertFalse(report["running"])
                    self.assertFalse(any(command[1] in ("enable", "preset", "start", "restart", "try-restart") for command in commands.calls))

    def test_upgrade_without_registry_can_restart_an_already_running_unit(self):
        for format in ("deb", "rpm", "arch"):
            with self.subTest(format=format), tempfile.TemporaryDirectory() as directory:
                commands = Commands(enabled="enabled", active=True)
                self.assertTrue(self.configure(format, commands, Path(directory) / "state.json", upgrade=True)["running"])
                action = ["deb-systemd-invoke", "restart"] if format == "deb" else ["systemctl", "try-restart"]
                self.assertIn([*action, common.ENGINE], commands.calls)
                self.assertFalse(any(command[1] in ("enable", "preset", "start") for command in commands.calls))

    def test_legacy_schema_one_without_removal_marker_is_not_a_fresh_install(self):
        for upgrade in (False, True):
            with self.subTest(upgrade=upgrade), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                common.write_json(status, {"schema": 1})
                commands = Commands()
                report = self.configure("deb", commands, status, upgrade=upgrade)
                self.assertEqual(report["status"], "disabled")
                self.assertFalse(any(command[0] == "deb-systemd-helper" for command in commands.calls))

    def test_remove_reinstall_restores_only_enabled_and_running_state_removed_by_package(self):
        for format in ("deb", "rpm", "arch"):
            for enabled in ("disabled", "enabled"):
                for active in (False, True):
                    with self.subTest(format=format, enabled=enabled, active=active), tempfile.TemporaryDirectory() as directory:
                        status = Path(directory) / "state.json"
                        commands = Commands()
                        self.configure(format, commands, status)
                        commands.enabled, commands.active = enabled, active
                        self.remove(format, commands, status)
                        removal = common.json_file(status)["removal"]
                        self.assertEqual(removal["enabled"], enabled == "enabled")
                        self.assertEqual(removal["running"], active)
                        self.assertFalse(commands.active)
                        commands.calls.clear()
                        # Debian supplies the previous version even after a real remove.
                        report = self.configure(format, commands, status, upgrade=format == "deb")
                        self.assertEqual(report["enabled"], enabled == "enabled")
                        self.assertEqual(report["running"], enabled == "enabled" and active)
                        self.assertNotIn("removal", common.json_file(status))
                        if enabled == "disabled":
                            self.assertFalse(any(command[1] in ("enable", "preset") for command in commands.calls))
                        if not active or enabled == "disabled":
                            self.assertFalse(any(command[1] in ("start", "restart", "try-restart") for command in commands.calls))

    def test_reinstall_respects_mask_preset_and_debian_startup_policy(self):
        for format in ("deb", "rpm", "arch"):
            for policy in ("mask", "startup"):
                with self.subTest(format=format, policy=policy), tempfile.TemporaryDirectory() as directory:
                    status = Path(directory) / "state.json"
                    commands = Commands(enabled="enabled", active=True)
                    self.remove(format, commands, status)
                    if policy == "mask":
                        commands.enabled = "masked"
                    elif format == "deb":
                        commands.policy = False
                    else:
                        commands.preset = False
                    commands.calls.clear()
                    report = self.configure(format, commands, status, upgrade=True)
                    self.assertFalse(report["running"])
                    self.assertEqual(report["status"], "masked" if policy == "mask" else "pending" if format == "deb" else "disabled")
                    self.assertNotIn("removal", common.json_file(status))
                    if policy == "mask":
                        self.assertFalse(any(command[1] in ("enable", "preset", "unmask", "start") for command in commands.calls))

    def test_incomplete_or_malformed_marker_cannot_authorize_reactivation(self):
        for removal in (True, {}, {"enabled": True, "running": True},
                        {"enabled": "true", "running": True, "disabled": True, "stopped": True},
                        {"enabled": True, "running": True, "disabled": False, "stopped": False}):
            with self.subTest(removal=removal), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                common.write_json(status, {"schema": 1, "engine": common.ENGINE, "removal": removal})
                commands = Commands()
                self.assertEqual(self.configure("deb", commands, status, upgrade=True)["status"], "disabled")
                self.assertFalse(any(command[0] == "deb-systemd-helper" for command in commands.calls))

    def test_failed_configure_preserves_marker_until_successful_retry(self):
        for failure in ("--check-imports", "start"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                commands = Commands(enabled="enabled", active=True)
                self.remove("deb", commands, status)
                removal = common.json_file(status)["removal"]
                commands.failures.add(failure)
                with self.assertRaises(common.PackagingError):
                    self.configure("deb", commands, status, upgrade=True)
                self.assertEqual(common.json_file(status)["removal"], removal)
                commands.failures.clear()
                self.assertTrue(self.configure("deb", commands, status, upgrade=True)["running"])
                self.assertNotIn("removal", common.json_file(status))

    def test_failed_configure_save_preserves_removal_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            commands = Commands(enabled="enabled", active=True)
            self.remove("deb", commands, status)
            previous = common.json_file(status)
            with patch.object(native, "write_json", side_effect=OSError("simulated save failure")), self.assertRaises(OSError):
                self.configure("deb", commands, status, upgrade=True)
            self.assertEqual(common.json_file(status), previous)

    def test_offline_install_configures_first_boot_without_starting_services(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands()
            report = self.configure("arch", commands, Path(directory) / "state.json", live=False)
            self.assertEqual(report["status"], "pending-boot")
            self.assertIn(["systemctl", "--root=/", "preset", common.ENGINE], commands.calls)
            self.assertIn([native.IMPORT_CHECK, "--check-imports"], commands.calls)
            self.assertFalse(any("start" in command for command in commands.calls))

    def test_offline_remove_reinstall_restores_enablement_but_never_claims_running(self):
        for format in ("deb", "rpm", "arch"):
            with self.subTest(format=format), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                commands = Commands(enabled="enabled")
                self.remove(format, commands, status, live=False)
                self.assertIsNone(common.json_file(status)["removal"]["running"])
                commands.calls.clear()
                report = self.configure(format, commands, status, upgrade=True, live=False)
                self.assertTrue(report["enabled"])
                self.assertEqual(report["status"], "pending-boot")
                self.assertFalse(report["running"])
                self.assertFalse(any(command[1] in ("start", "stop", "is-active", "try-restart") for command in commands.calls))

    def test_offline_upgrade_without_registry_preserves_disabled_state(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands()
            report = self.configure("arch", commands, Path(directory) / "state.json", upgrade=True, live=False)
            self.assertFalse(report["enabled"])
            self.assertFalse(report["running"])
            self.assertFalse(any("preset" in command or "start" in command for command in commands.calls))

    def test_unknown_state_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            common.write_json(status, {"schema": 2})
            with self.assertRaises(common.PackagingError):
                self.configure("deb", Commands(), status)
            self.assertEqual(common.json_file(status), {"schema": 2})

    def test_remove_only_stops_private_backend(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands(enabled="enabled", active=True)
            native.remove("deb", commands, True, lambda name: name, status=Path(directory) / "state.json")
            self.assertIn(["systemctl", "stop", common.ENGINE], commands.calls)
            self.assertFalse(commands.active)
            self.assertFalse(any("--user" in command for command in commands.calls))

    def test_remove_preserves_administrator_mask(self):
        with tempfile.TemporaryDirectory() as directory:
            commands = Commands(enabled="masked")
            self.remove("deb", commands, Path(directory) / "state.json")
            self.assertEqual(commands.enabled, "masked")
            self.assertFalse(any(command[1] in ("disable", "unmask") for command in commands.calls))

    def test_remove_errors_are_reported_and_successful_transitions_survive_retry(self):
        for failure in ("disable", "stop"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                commands = Commands(enabled="enabled", active=True, failures=(failure,))
                with self.assertRaises(common.PackagingError):
                    self.remove("deb", commands, status)
                report = common.json_file(status)
                self.assertEqual(report["status"], "remove-failed")
                self.assertTrue(report["pending"])
                self.assertTrue(report["removal"]["enabled"])
                self.assertTrue(report["removal"]["running"])
                self.assertEqual(report["removal"]["disabled"], failure != "disable")
                self.assertEqual(report["removal"]["stopped"], failure != "stop")
                commands.failures.clear()
                self.remove("deb", commands, status)
                self.assertTrue(self.configure("deb", commands, status, upgrade=True)["running"])

    def test_remove_retry_does_not_claim_an_administrators_later_disable_or_stop(self):
        for failure in ("disable", "stop"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                status = Path(directory) / "state.json"
                commands = Commands(enabled="enabled", active=True, failures=(failure,))
                with self.assertRaises(common.PackagingError):
                    self.remove("deb", commands, status)
                commands.failures.clear()
                if failure == "disable":
                    commands.enabled = "disabled"
                else:
                    commands.active = False
                self.remove("deb", commands, status)
                removal = common.json_file(status)["removal"]
                self.assertFalse(removal["disabled" if failure == "disable" else "stopped"])
                report = self.configure("deb", commands, status, upgrade=True)
                self.assertEqual(report["enabled"], failure == "stop")
                self.assertFalse(report["running"])

    def test_remove_save_failure_happens_before_service_mutations(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            common.write_json(status, {"schema": 1})
            commands = Commands(enabled="enabled", active=True)
            with patch.object(native, "write_json", side_effect=OSError("simulated save failure")), self.assertRaises(OSError):
                self.remove("deb", commands, status)
            self.assertEqual(common.json_file(status), {"schema": 1})
            self.assertFalse(any(command[1] in ("disable", "stop") for command in commands.calls))

    def test_remove_final_save_failure_keeps_a_repairable_checkpoint(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            commands = Commands(enabled="enabled", active=True)
            writes = 0

            def save(path, record):
                nonlocal writes
                writes += 1
                if writes == 3:
                    raise OSError("simulated final save failure")
                common.write_json(path, record)

            with patch.object(native, "write_json", side_effect=save), self.assertRaises(OSError):
                self.remove("deb", commands, status)
            removal = common.json_file(status)["removal"]
            self.assertTrue(removal["enabled"])
            self.assertTrue(removal["running"])
            self.assertTrue(removal["disabled"])
            self.assertFalse(removal["stopped"])
            self.assertFalse(commands.active)
            self.assertFalse(self.configure("deb", commands, status, upgrade=True)["running"])

    def test_remove_unknown_schema_is_preserved_without_service_mutations(self):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "state.json"
            common.write_json(status, {"schema": 2})
            commands = Commands(enabled="enabled", active=True)
            with self.assertRaises(common.PackagingError):
                self.remove("deb", commands, status)
            self.assertEqual(common.json_file(status), {"schema": 2})
            self.assertEqual(commands.calls, [])

    def test_core_native_dependencies_include_audio_and_mouse_backends(self):
        for target in common.load_targets()["targets"]:
            captured = []
            def resolve(actual, requirements):
                captured.extend(requirements)
                return [{"name": common.package_name(value), "requirement": value, "version": "1"} for value in requirements], []
            with self.subTest(target=target["id"]), patch("build.resolve_packages", side_effect=resolve):
                native_requirements(target)
                self.assertTrue(all(value in captured for value in target["audio"] + target["ratbag"]))
                if target["format"] == "deb":
                    self.assertIn("init-system-helpers", captured)

    def test_installed_hooks_delegate_no_package_manager_or_root_user_service(self):
        for relative in ("deb/postinst", "deb/prerm", "arch/peripheral-hub.install", "rpm/peripheral-hub.spec"):
            content = (PACKAGING / relative).read_text()
            self.assertIn("/usr/lib/peripheral-hub/installer/native.py", content)
            self.assertNotIn("apt-get", content)
            self.assertNotIn("sudo", content)
            self.assertNotIn("systemctl --user", content)


if __name__ == "__main__":
    unittest.main()
