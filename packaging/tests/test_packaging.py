import copy
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from datetime import date
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import common
import install
import sources
import staging
import system
from build import app_version, write_manifest
from ci import smoke
from ci.toolchain import extract_node


def fixture(name):
    value = common.json_file(PACKAGING / "tests/fixtures" / (name + ".json"))
    target = common.target_for(common.parse_os_release(value["os_release"]), today=date(2026, 11, 1))
    return target, value["facts"]


def all_consents():
    return dict(audio=True, openrazer=True, audio_switch=True, session_manager=True, ratbag=True, repository=True, unsigned_dkms=True)


def source_tar(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, kind in entries:
            member = tarfile.TarInfo(name)
            if kind == "file":
                member.size = 3
                archive.addfile(member, io.BytesIO(b"abc"))
            else:
                member.type = tarfile.SYMTYPE if kind == "symlink" else tarfile.LNKTYPE
                member.linkname = "../../outside"
                archive.addfile(member)
    return output.getvalue()


class TargetTests(unittest.TestCase):
    def test_actual_catalog_covers_only_explicit_targets(self):
        catalog = common.load_targets()
        self.assertEqual(len(catalog["targets"]), 10)
        for target in catalog["targets"]:
            release = {"ID": target["os_id"]}
            if not target.get("rolling"):
                release["VERSION_ID"] = target["version_ids"][0]
            self.assertEqual(common.target_for(release, catalog, date(2026, 11, 1))["id"], target["id"])
            self.assertNotIn("input-remapper", target["runtime"])

    def test_inheritance_preserves_child_release_and_manager(self):
        targets = {item["id"]: item for item in common.load_targets()["targets"]}
        self.assertIn("libgtk-3-0t64", targets["ubuntu-24.04"]["runtime"])
        self.assertIn("libgtk-3-0t64", targets["ubuntu-26.04"]["runtime"])
        self.assertIn("libgtk-3-0t64", targets["debian-13"]["runtime"])
        self.assertEqual(targets["debian-12"]["repository"]["kind"], "obs-deb")
        self.assertEqual(targets["cachyos"]["os_id"], "cachyos")

    def test_id_like_is_not_support(self):
        with self.assertRaises(common.PackagingError):
            common.target_for({"ID": "linuxmint", "ID_LIKE": "ubuntu", "VERSION_ID": "22"})

    def test_unsupported_releases_and_immutable_families(self):
        for release in ({"ID": "debian", "VERSION_ID": "11"}, {"ID": "ubuntu", "VERSION_ID": "20.04"}, {"ID": "fedora", "VERSION_ID": "rawhide"}, {"ID": "opensuse-leap", "VERSION_ID": "15.6"}, {"ID": "nixos"}, {"ID": "alpine"}):
            with self.subTest(release=release), self.assertRaises(common.PackagingError):
                common.target_for(release)

    def test_fedora_support_expires_without_silently_selecting_rawhide(self):
        with self.assertRaisesRegex(common.PackagingError, "expired"):
            common.target_for({"ID": "fedora", "VERSION_ID": "44"}, today=date(2027, 5, 2))

    def test_os_release_is_data_not_a_shell_program(self):
        value = common.parse_os_release('ID=ubuntu\nVERSION_ID="24.04"\nNAME="literal $(false)"\n')
        self.assertEqual(value["NAME"], "literal $(false)")
        for text in ("ID=arch\nID=ubuntu", "ID=ubuntu; false", 'ID="ubuntu', "ID_LIKE=arch", "invalid"):
            with self.subTest(text=text), self.assertRaises(common.PackagingError):
                common.parse_os_release(text)

    def test_root_arch_init_and_immutable_preflight(self):
        target, facts = fixture("cachyos")
        install.preflight(target, facts)
        for field, value in (("uid", 0), ("arch", "aarch64"), ("init", "openrc"), ("immutable", True), ("python", "3.9.2")):
            invalid = dict(facts, **{field: value})
            with self.subTest(field=field), self.assertRaises(common.PackagingError):
                install.preflight(target, invalid)


class KernelTests(unittest.TestCase):
    def test_cachyos_uses_real_pkgbase_not_generic_linux(self):
        target, facts = fixture("cachyos")
        self.assertEqual(common.kernel_packages(target, facts), ["linux-cachyos-headers"])
        facts["kernel_pkgbase"] = "linux-cachyos-lts"
        self.assertEqual(common.kernel_packages(target, facts), ["linux-cachyos-lts-headers"])

    def test_arch_does_not_guess_unavailable_pkgbase(self):
        target, facts = fixture("cachyos")
        for bad in ("", "../linux", "linux;sh", "$(uname)"):
            with self.subTest(value=bad), self.assertRaises(common.PackagingError):
                common.kernel_packages(target, dict(facts, kernel_pkgbase=bad))

    def test_existing_matching_headers_need_no_other_kernel(self):
        for name in ("cachyos", "ubuntu22", "debian12", "fedora44", "opensuse16"):
            target, facts = fixture(name)
            self.assertEqual(common.kernel_packages(target, dict(facts, kernel_headers_match=True)), [])

    def test_debian_ubuntu_headers_use_uname_not_latest_metapackage(self):
        for name in ("ubuntu22", "debian12"):
            target, facts = fixture(name)
            self.assertEqual(common.kernel_packages(target, facts), ["linux-headers-" + facts["kernel"]])

    def test_fedora_uses_exact_non_debug_devel(self):
        target, facts = fixture("fedora44")
        self.assertEqual(common.kernel_packages(target, facts), ["kernel-devel-" + facts["kernel"]])
        with self.assertRaises(common.PackagingError):
            common.kernel_packages(target, dict(facts, kernel="6.18.2-debug.x86_64"))

    def test_opensuse_uses_exact_kernel_flavor_and_package_version(self):
        target, facts = fixture("opensuse16")
        self.assertEqual(common.kernel_packages(target, facts), ["kernel-default-devel=6.12.0-160000.9.1", "kernel-devel=6.12.0-160000.9.1"])

    def test_untrusted_kernel_strings_cannot_be_command_arguments(self):
        target, facts = fixture("debian12")
        with self.assertRaises(common.PackagingError):
            common.kernel_packages(target, dict(facts, kernel="6.1; reboot"))


class PlanTests(unittest.TestCase):
    def test_every_distro_has_separate_root_and_normal_user_phases(self):
        for name in ("cachyos", "ubuntu22", "debian12", "fedora44", "opensuse16"):
            target, facts = fixture(name)
            plan = install.system_plan(target, facts, all_consents())
            for operation in plan["commands"]:
                self.assertEqual(operation["scope"], "root")
                self.assertNotIn("--user", operation["argv"])
            for operation in install.user_commands(plan, facts, all_consents()):
                self.assertEqual(operation["scope"], "user")
                if "argv" in operation and operation["argv"][0] == "systemctl":
                    self.assertIn("--user", operation["argv"])

    def test_audio_and_ratbag_refusal_is_not_complete_success(self):
        target, facts = fixture("ubuntu22")
        choices = dict(all_consents(), audio_switch=False, session_manager=False, ratbag=False)
        plan = install.system_plan(target, facts, choices)
        self.assertTrue(any(item.startswith("audio:") for item in plan["pending"]))
        self.assertTrue(any(item.startswith("ratbag:") for item in plan["pending"]))
        self.assertNotIn("pipewire-pulse", plan["packages"])
        self.assertNotIn("wireplumber", plan["packages"])
        self.assertNotIn("ratbagd", plan["packages"])

    def test_secure_boot_requires_acknowledgement_and_stays_pending(self):
        target, facts = fixture("ubuntu22")
        refused = install.system_plan(target, facts, dict(all_consents(), unsigned_dkms=False))
        self.assertNotIn("dkms", refused["packages"])
        accepted = install.system_plan(target, facts, all_consents())
        self.assertIn("dkms", accepted["packages"])
        self.assertTrue(any("MOK" in item for item in accepted["pending"]))
        self.assertFalse(any("mokutil" in operation["argv"] for operation in accepted["commands"]))

    def test_current_arch_openrazer_needs_no_aur_or_upstream_repo(self):
        target, facts = fixture("cachyos")
        plan = install.system_plan(target, facts, all_consents())
        self.assertIsNone(plan["repository"])
        text = [argument for operation in plan["commands"] for argument in operation["argv"]]
        for forbidden in ("yay", "paru", "pip", "curl", "input-remapper.service"):
            self.assertNotIn(forbidden, text)

    def test_refused_upstream_repo_does_not_attempt_an_old_driver_install(self):
        target, facts = fixture("debian12")
        plan = install.system_plan(target, facts, dict(all_consents(), repository=False))
        self.assertIsNone(plan["repository"])
        self.assertNotIn("openrazer-driver-dkms", plan["packages"])
        self.assertTrue(any("repository" in item for item in plan["pending"]))

    def test_current_native_driver_needs_no_extra_repository_consent(self):
        target, facts = fixture("debian12")
        facts["openrazer_repository_version"] = "3.12.4-1"
        plan = install.system_plan(target, facts, dict(all_consents(), repository=False))
        self.assertIsNone(plan["repository"])
        self.assertIn("openrazer-driver-dkms", plan["packages"])
        self.assertFalse(any("repository" in item for item in plan["pending"]))

    def test_uninstall_touches_only_private_services_and_app_package(self):
        target, facts = fixture("debian12")
        plan = install.system_plan(target, facts, all_consents(), "uninstall")
        self.assertEqual(plan["commands"][0]["argv"], ["systemctl", "disable", "--now", common.ENGINE])
        self.assertEqual(plan["commands"][1]["argv"], ["apt-get", "remove", common.APP])
        self.assertEqual(plan["packages"], [])
        text = [argument for operation in plan["commands"] for argument in operation["argv"]]
        for external in ("openrazer-daemon.service", "ratbagd.service", "pipewire.service", "input-remapper.service", "--autoremove"):
            self.assertNotIn(external, text)
        recovery = install.user_commands(plan, facts, all_consents())[0]["argv"]
        self.assertEqual(recovery, ["/usr/bin/peripheral-hub-remapping-agent", "--disable-all"])

    def test_missing_headers_is_explicit_pending_not_a_guessed_kernel(self):
        target, facts = fixture("cachyos")
        plan = install.system_plan(target, dict(facts, kernel_pkgbase=""), all_consents())
        self.assertTrue(any("pkgbase" in item for item in plan["pending"]))
        self.assertNotIn("linux-headers", plan["packages"])

    def test_static_user_services_are_started_not_enabled(self):
        def response(argv, **kwargs):
            return SimpleNamespace(returncode=0, stdout="loaded\n" if "show" in argv else "static\n" if "is-enabled" in argv else "", stderr="")
        with patch.object(install, "run", side_effect=response) as mocked:
            install.activate_user_unit("openrazer-daemon.service")
        self.assertEqual(mocked.call_args.args[0], ["systemctl", "--user", "start", "openrazer-daemon.service"])

    def test_masked_user_services_are_preserved(self):
        with patch.object(install, "run", side_effect=[SimpleNamespace(returncode=0, stdout="loaded\n"), SimpleNamespace(returncode=1, stdout="masked\n")]):
            with self.assertRaisesRegex(common.PackagingError, "masked"):
                install.activate_user_unit("wireplumber.service")

    def test_native_repair_reinstalls_and_does_not_allow_unapproved_removal(self):
        target, _ = fixture("debian12")
        with patch.object(system, "run", return_value=SimpleNamespace(stdout="", stderr="", returncode=0)) as mocked:
            system.native_install(target, Path("/verified/app.deb"), [], all_consents(), repair=True)
            self.assertIn("--reinstall", mocked.call_args.args[0])
        with patch.object(system, "run", return_value=SimpleNamespace(stdout="Remv unrelated-package\n", stderr="", returncode=0)):
            with self.assertRaisesRegex(common.PackagingError, "unapproved"):
                system.native_install(target, Path("/verified/app.deb"), [], all_consents())

    def test_native_install_refuses_to_add_a_different_kernel(self):
        target, _ = fixture("debian12")
        with patch.object(system, "run", return_value=SimpleNamespace(stdout="Inst linux-image-6.19-amd64\n", stderr="", returncode=0)):
            with self.assertRaisesRegex(common.PackagingError, "kernel"):
                system.native_install(target, Path("/verified/app.deb"), [], all_consents())

    def test_rpm_repository_signatures_must_remain_enabled(self):
        target, _ = fixture("fedora44")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = root / "yum.repos.d"
            repos.mkdir()
            path = repos / "fixture.repo"
            path.write_text("[fixture]\nenabled=1\ngpgcheck=0\n")
            with self.assertRaisesRegex(common.PackagingError, "disabled"):
                system.check_repository_policy(target, root)
            path.write_text("[fixture]\nenabled=1\ngpgcheck=1\n")
            system.check_repository_policy(target, root)

    def test_administrative_bootstrap_compiles_and_never_imports_unverified_helper(self):
        compile(install.ROOT_BOOTSTRAP, "<root-bootstrap>", "exec")
        self.assertLess(install.ROOT_BOOTSTRAP.index("Administrative artifact checksum changed"), install.ROOT_BOOTSTRAP.index('str(destination / "system.py")'))
        self.assertNotIn("--user", install.ROOT_BOOTSTRAP)


class IntegrityTests(unittest.TestCase):
    def make_bundle(self, root):
        for name in ("install.py", "system.py", "common.py", "targets.json", "app.deb", "source.tar.gz"):
            (root / name).write_bytes(b"verified contents")
        manifest = {
            "schema": 1, "app": {"name": "peripheral-hub", "version": "0.1.0"}, "architecture": "x86_64", "init": "systemd",
            "target": {"id": "debian-12", "os_release": {"ID": "debian", "VERSION_ID": "12"}, "image_digest": "sha256:" + "a" * 64},
            "package": {"file": "app.deb", "format": "deb"}, "sources": [{"file": "source.tar.gz"}],
            "validation": {"native_smoke": "passed"},
        }
        write_manifest(root, manifest)
        return manifest

    def test_manifest_and_every_artifact_are_verified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_bundle(root)
            manifest, checksum = common.verify_bundle(root)
            self.assertEqual(manifest["app"]["version"], "0.1.0")
            self.assertEqual(len(checksum), 64)
            (root / "app.deb").write_bytes(b"tampered package")
            with self.assertRaisesRegex(common.PackagingError, "checksum"):
                common.verify_bundle(root)

    def test_manifest_tampering_and_root_phase_cas_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_bundle(root)
            with self.assertRaisesRegex(common.PackagingError, "changed"):
                common.verify_bundle(root, "b" * 64)
            (root / "manifest.json").write_text("{}")
            with self.assertRaisesRegex(common.PackagingError, "Manifest SHA256"):
                common.verify_bundle(root)

    def test_path_traversal_and_special_artifacts_rejected(self):
        for name in ("../file", "/etc/passwd", "sub/../../file", "a\\file", "a//file", "a/./file", "C:\\host", "C:/host", "root/C:/host", ""):
            with self.subTest(name=name), self.assertRaises(common.PackagingError):
                common.safe_relative(name)

    def test_hardlinks_not_accepted_as_verified_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "file").write_bytes(b"data")
            os.link(root / "file", root / "alias")
            with self.assertRaises(common.PackagingError):
                common.regular_bytes(root / "file")

    def test_native_smoke_gate_and_wrong_target_are_rejected(self):
        target, facts = fixture("debian12")
        with tempfile.TemporaryDirectory() as temporary:
            manifest = self.make_bundle(Path(temporary))
            install.preflight(target, facts, manifest)
            manifest["validation"]["native_smoke"] = "not-run"
            with self.assertRaisesRegex(common.PackagingError, "smoke"):
                install.preflight(target, facts, manifest)
            manifest["target"]["id"] = "debian-13"
            with self.assertRaisesRegex(common.PackagingError, "different"):
                install.preflight(target, facts, manifest)

    def test_smoke_service_state_gate_rejects_missing_units_and_state_mismatches(self):
        for enabled, active in ((False, False), (True, False), (True, True)):
            expected = [SimpleNamespace(stdout="enabled\n" if enabled else "disabled\n", returncode=0 if enabled else 1),
                        SimpleNamespace(stdout="active\n" if active else "inactive\n", returncode=0 if active else 3)]
            with self.subTest(enabled=enabled, active=active), patch.object(smoke, "run", side_effect=expected):
                smoke.verify_service_state(enabled, active, "fixture")
            with self.subTest(mismatch=enabled), patch.object(smoke, "run", side_effect=expected), self.assertRaises(common.PackagingError):
                smoke.verify_service_state(not enabled, active, "fixture")
            with self.subTest(mismatch=active), patch.object(smoke, "run", side_effect=expected), self.assertRaises(common.PackagingError):
                smoke.verify_service_state(enabled, not active, "fixture")
        missing = [SimpleNamespace(stdout="not-found\n", returncode=1), SimpleNamespace(stdout="unknown\n", returncode=4)]
        with patch.object(smoke, "run", side_effect=missing), self.assertRaises(common.PackagingError):
            smoke.verify_service_state(False, False, "fixture")

    def test_atomic_status_update_can_repair_a_previous_failed_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "state.json"
            path.with_suffix(".json.tmp").write_text("stale incomplete write")
            common.write_json(path, {"status": "installed"})
            common.write_json(path, {"status": "pending"})
            self.assertEqual(common.json_file(path)["status"], "pending")

    def test_owned_configuration_is_idempotent_and_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "owned.repo"
            record = {}
            system.owned_file(path, b"signed repo\n", record)
            system.owned_file(path, b"signed repo\n", record)
            with self.assertRaisesRegex(common.PackagingError, "not overwriting"):
                system.owned_file(path, b"other settings\n", record)
            self.assertEqual(path.read_bytes(), b"signed repo\n")

    def test_source_extraction_blocks_links_and_traversal(self):
        for entries in ([('root/../../outside', 'file')], [('root/link', 'symlink')], [('root/link', 'hardlink')], [('root/a', 'file'), ('root/a', 'file')]):
            with tempfile.TemporaryDirectory() as temporary, self.assertRaises(common.PackagingError):
                sources.extract_source(source_tar(entries), temporary)

    def test_real_source_extraction_has_contents(self):
        with tempfile.TemporaryDirectory() as temporary:
            sources.extract_source(source_tar([("root/src/main.c", "file")]), temporary)
            self.assertEqual((Path(temporary) / "src/main.c").read_bytes(), b"abc")

    def test_toolchain_extraction_works_without_python_3_12_tar_filter(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "node.tar.xz"
            with tarfile.open(archive, "w:xz") as source:
                member = tarfile.TarInfo("node-test/bin/node")
                member.size = 3
                member.mode = 0o755
                source.addfile(member, io.BytesIO(b"abc"))
            extract_node(archive, root / "extracted", "node-test")
            self.assertEqual((root / "extracted/node-test/bin/node").read_bytes(), b"abc")

    def test_toolchain_extraction_rejects_symlinks_outside_its_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "node.tar.xz"
            with tarfile.open(archive, "w:xz") as source:
                member = tarfile.TarInfo("node-test/bin/npm")
                member.type = tarfile.SYMTYPE
                member.linkname = "../../outside"
                source.addfile(member)
            with self.assertRaises(common.PackagingError):
                extract_node(archive, root / "extracted", "node-test")

    def test_all_version_manifests_stay_at_approved_0_1_0(self):
        self.assertEqual(app_version(), "0.1.0")

    def test_rootfs_includes_real_user_unit_and_recovery(self):
        unit = (PACKAGING / "rootfs/usr/lib/systemd/user/peripheral-hub-remapping-agent.service").read_text()
        desktop = (PACKAGING / "rootfs/usr/share/applications/io.peripheralhub.Recovery.desktop").read_text()
        self.assertIn("ExecStart=/usr/bin/peripheral-hub-remapping-agent\n", unit)
        self.assertIn("Exec=/usr/bin/peripheral-hub-remapping-agent --disable-all", desktop)
        self.assertNotIn("sudo", unit + desktop)
        autostart = (PACKAGING / "rootfs/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop").read_text()
        self.assertIn("Exec=systemctl --user start peripheral-hub-remapping-agent.service", autostart)
        self.assertNotIn("sudo", autostart)

    def test_arch_autostart_is_in_staged_payload_and_has_native_backup_metadata(self):
        relative = "etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop"
        recipe = (PACKAGING / "arch/PKGBUILD").read_text()
        self.assertIn("backup=('" + relative + "')", recipe)
        self.assertNotIn("backup=('/", recipe)
        self.assertIn('cp -a "$srcdir/etc" "$pkgdir/etc"', recipe)
        self.assertIn(relative, staging.REQUIRED)
        with tempfile.TemporaryDirectory() as directory:
            root, archive = Path(directory) / "rootfs", Path(directory) / "rootfs.tar.gz"
            staging.merge_rootfs(PACKAGING / "rootfs", root)
            staging.rootfs_archive(root, archive, 0)
            with tarfile.open(archive, "r:gz") as payload:
                self.assertEqual(payload.extractfile(relative).read(), (PACKAGING / "rootfs" / relative).read_bytes())


class HeadsetStagingTests(unittest.TestCase):
    def stage_candidate(self, directory, mode, verified=None, license_name="license", target_id=None, configured=None):
        root = Path(directory)
        rootfs, work, artifacts = root / "rootfs", root / "work", root / "artifacts"
        (rootfs / "usr/lib/peripheral-hub").mkdir(parents=True)
        work.mkdir()
        artifacts.mkdir()
        entries = [("HeadsetControl-4.2.0/src/main.c", "file")]
        if license_name:
            entries.append(("HeadsetControl-4.2.0/" + license_name, "file"))
        source = source_tar(entries)
        zipped = io.BytesIO()
        with zipfile.ZipFile(zipped, "w") as archive:
            archive.writestr("headsetcontrol", b"private CLI")
        binary = work / "headsetcontrol-build/headsetcontrol"

        def command(argv, **kwargs):
            if argv[:2] == ["cmake", "-S"] and configured is not None:
                configured.append(argv)
            if argv[:2] == ["cmake", "--build"]:
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_bytes(b"private CLI")
            return SimpleNamespace(stdout="private udev rules\n", stderr="", returncode=0)

        verified = verified if verified is not None else {"configured": True, "reason": "CLI schema 1.6 verified"}
        with patch.object(sources, "require_build_user"), \
                patch.object(sources, "fetch_locked", side_effect=[source, zipped.getvalue()]), \
                patch.object(sources, "run", side_effect=command), \
                patch.object(staging, "elf_report"), \
                patch.object(sources, "probe_headset", return_value=verified), \
                patch.object(sources.shutil, "copyfile", wraps=sources.shutil.copyfile) as copied:
            result = sources.headset_candidate(rootfs, {"id": target_id, "headset_build": mode}, work, artifacts, root / "cache")
        return result, rootfs, source, copied.call_args_list

    def test_debian_12_uses_clang_and_libcxx_without_changing_debian_13(self):
        for target in ("debian-12", "debian-13"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                configured = []
                result, *_ = self.stage_candidate(directory, "source", target_id=target, configured=configured)
                self.assertTrue(result["private"])
                self.assertEqual(len(configured), 1)
                for arg in ("-DCMAKE_CXX_COMPILER=clang++-19", "-DCMAKE_CXX_FLAGS=-stdlib=libc++ -static-libstdc++"):
                    self.assertEqual(arg in configured[0], target == "debian-12")

    def test_private_candidate_stages_exact_upstream_lowercase_license_for_both_build_paths(self):
        for mode in ("source", "prebuilt"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                result, rootfs, source, copied = self.stage_candidate(directory, mode)
                self.assertTrue(result["configured"])
                self.assertTrue(result["private"])
                self.assertEqual(result["version"], "4.2.0")
                self.assertEqual(result["selection"], "private")
                self.assertEqual((rootfs / "usr/lib/peripheral-hub/headsetcontrol").read_bytes(), b"private CLI")
                docs = rootfs / "usr/share/doc/peripheral-hub/headsetcontrol"
                self.assertEqual((docs / "LICENSE").read_bytes(), b"abc")
                self.assertEqual((docs / result["source"]).read_bytes(), source)
                self.assertEqual((rootfs / "usr/lib/udev/rules.d/70-peripheral-hub-headsets.rules").read_text(), "private udev rules\n")
                license_copies = [call.args for call in copied if Path(call.args[1]).name == "LICENSE"]
                self.assertEqual(len(license_copies), 1)
                # Path equality is case-insensitive on Windows; inspect the original name.
                self.assertEqual(Path(license_copies[0][0]).name, "license")

    def test_missing_license_is_a_staging_error_not_complete_success(self):
        with tempfile.TemporaryDirectory() as directory, self.assertRaises(OSError):
            self.stage_candidate(directory, "prebuilt", license_name=None)

    def test_unconfigured_candidate_diagnostic_uses_verified_api(self):
        with tempfile.TemporaryDirectory() as directory:
            result, rootfs, _, _ = self.stage_candidate(directory, "prebuilt", {"configured": False})
            self.assertFalse(result["private"])
            self.assertIn("API 1.6", result["reason"])
            self.assertNotIn("1.8/1.9", result["reason"])
            self.assertNotIn("path", result)
            self.assertFalse((rootfs / "usr/lib/peripheral-hub/headsetcontrol").exists())


class ContractTests(unittest.TestCase):
    def test_headset_only_verified_upstream_api_strings(self):
        for api in ("1.6",):
            self.assertTrue(common.headset_contract(json.dumps({"name": "HeadsetControl", "api_version": api, "devices": [], "device_count": 0}).encode())[0])
        for api in ("1.8", "1.9", "3.0", 1.6, 3, None):
            self.assertFalse(common.headset_contract(json.dumps({"name": "HeadsetControl", "api_version": api, "devices": [], "device_count": 0}).encode())[0])

    def test_headset_official_capabilities_without_invented_readable_array(self):
        value = common.json_file(PACKAGING / "tests/fixtures/headsetcontrol-4.2.json")
        value["devices"] = value["devices"][:1]
        value["device_count"] = 1
        capabilities = value["devices"][0].pop("capabilities")
        self.assertFalse(common.headset_contract(json.dumps(value).encode())[0])
        value["devices"][0]["capabilities"] = capabilities
        self.assertTrue(common.headset_contract(json.dumps(value).encode())[0])

    def test_headset_count_and_provider_limits_match_rust(self):
        self.assertFalse(common.headset_contract(b" " * (256 * 1024 + 1))[0])
        self.assertFalse(common.headset_contract(json.dumps({"name": "HeadsetControl", "api_version": "1.6", "devices": [], "device_count": 1}).encode())[0])
        self.assertFalse(common.headset_contract(json.dumps({"name": "HeadsetControl", "api_version": "1.6", "devices": []}).encode())[0])
        self.assertFalse(common.headset_contract(json.dumps({"name": "HeadsetControl", "api_version": "1.6", "devices": [None] * 257, "device_count": 257}).encode())[0])

    def test_headset_upstream_empty_inventory_exit_one_verifies_software_only(self):
        def empty_inventory(argv, **kwargs):
            kwargs["stdout"].write(b'{"name":"HeadsetControl","api_version":"1.6","device_count":0,"devices":[]}')
            return SimpleNamespace(returncode=1)
        with patch.object(common.subprocess, "run", side_effect=empty_inventory):
            result = common.probe_headset("/private/headsetcontrol")
        self.assertTrue(result["configured"])

    def test_headset_arbitrary_error_is_not_an_empty_inventory_success(self):
        def failed(argv, **kwargs):
            kwargs["stdout"].write(b'{"name":"HeadsetControl","api_version":"1.6","device_count":0,"devices":[]}')
            return SimpleNamespace(returncode=2)
        with patch.object(common.subprocess, "run", side_effect=failed):
            self.assertFalse(common.probe_headset("/private/headsetcontrol")["configured"])

    def test_headset_candidate_lock_does_not_claim_conformity(self):
        lock = common.json_file(PACKAGING / "headsetcontrol.lock.json")
        self.assertTrue(lock["candidate_only"])
        self.assertEqual(lock["accepted_api_versions"], ["1.6"])
        self.assertEqual(lock["commit"], "91ee690a5e302a10a6f4f75a46be05846a38a3b0")

    def test_pinned_evdev_private_is_source_only_not_global_pip(self):
        entry = common.json_file(PACKAGING / "python-sources.lock.json")["evdev"]
        self.assertEqual(entry["version"], "1.9.2")
        self.assertEqual(len(entry["sha256"]), 64)
        self.assertNotIn("pip", (PACKAGING / "ci/bootstrap.sh").read_text())

    def test_opensuse_python_flavor_is_resolved_from_native_metadata(self):
        target, _ = fixture("opensuse16")
        def query(manager, name):
            return "1.9.2" if name == "python313-evdev" else None
        resolved, missing = common.resolve_packages(target, ["python3-evdev >= 1.6.1"], query, (3, 13))
        self.assertEqual(missing, [])
        self.assertEqual(resolved[0]["name"], "python313-evdev")

    def test_opensuse_wrong_kernel_devel_version_cannot_be_substituted(self):
        target, _ = fixture("opensuse16")
        resolved, missing = common.resolve_packages(target, ["kernel-default-devel=6.12.0-160000.9.1"], lambda manager, name: "6.18.0-1")
        self.assertEqual(resolved, [])
        self.assertEqual(missing, ["kernel-default-devel"])

    def test_unsafe_package_arguments_and_missing_repos_rejected(self):
        for name in ("pkg; reboot", "--nogpgcheck", "$(curl remote)", "../package"):
            with self.subTest(name=name), self.assertRaises(common.PackagingError):
                common.package_name(name)
        target, _ = fixture("debian12")
        self.assertEqual(common.resolve_packages(target, ["python3-dasbus"], lambda manager, name: None), ([], ["python3-dasbus"]))

    def test_pgp_key_pin_rejects_malformed_key(self):
        with self.assertRaises(common.PackagingError):
            common.pgp_fingerprint(b"not a repository key")

    def test_pgp_fingerprint_matches_existing_upstream_primary_key(self):
        armored = common.regular_bytes(PACKAGING / "tests/fixtures/obs-hardware.asc")
        self.assertEqual(common.pgp_fingerprint(armored), "E96C496E4E77C159C2FCA053062F9FD4D6D11CE4")

    def test_cli_dry_run_on_windows_uses_only_fixture_and_never_elevation(self):
        result = subprocess.run([sys.executable, str(PACKAGING / "install.py"), "--dry-run", "--json", "--fixture", str(PACKAGING / "tests/fixtures/cachyos.json"), "--consent-ratbag"], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        plan = json.loads(result.stdout)
        self.assertEqual(plan["target"], "cachyos")
        self.assertNotIn("sudo", result.stdout)

    def test_real_fixture_mode_is_never_accepted(self):
        result = subprocess.run([sys.executable, str(PACKAGING / "install.py"), "--fixture", str(PACKAGING / "tests/fixtures/cachyos.json")], text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Fixtures cannot control", result.stderr)


if __name__ == "__main__":
    unittest.main()
