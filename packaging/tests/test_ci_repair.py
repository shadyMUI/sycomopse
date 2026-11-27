# SPDX-License-Identifier: MIT
"""Hermetic CI/build regressions; mocks never certify native services or hardware."""

import contextlib
import hashlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import build
import common
import stage_sidecars
from ci import dependencies, gates, matrix, smoke, toolchain
from ci.vm import SOFTWARE_GATES


def values(argv, flag):
    return [argv[index + 1] for index, argument in enumerate(argv[:-1]) if argument == flag]


class SmokeIsolationTests(unittest.TestCase):
    def test_command_is_rootless_delegated_and_has_only_private_namespaces(self):
        command = matrix.smoke_command(["podman"], "fixture", "image", PurePosixPath("/repo"), PurePosixPath("/repo/out"), "sha256:" + "a" * 64)
        self.assertEqual(command[:7], ["systemd-run", "--user", "--scope", "--quiet", "--property=Delegate=yes", "--", "podman"])
        for flag, expected in (("--user", "0:0"), ("--cgroupns", "private"), ("--pid", "private"),
                               ("--ipc", "private"), ("--network", "private")):
            self.assertEqual(values(command[7:], flag), [expected])
        self.assertEqual(values(command, "--volume"), ["/repo:/workspace:ro", "/repo/out:/artifacts:rw"])
        self.assertEqual(values(command, "--cap-drop"), ["ALL"])
        self.assertIn("no-new-privileges", command)
        for forbidden in ("--privileged", "--userns", "--device", "--device-cgroup-rule", "SYS_MODULE", "SYS_RAWIO", "NET_ADMIN", "MKNOD"):
            self.assertNotIn(forbidden, command)

    def test_docker_has_no_systemd_privilege_fallback(self):
        with self.assertRaisesRegex(common.PackagingError, "Docker"):
            matrix.smoke_command(["docker"], "fixture", "image", Path("/repo"), Path("/repo/out"), "sha256:" + "a" * 64)

    def test_flatpak_wrapper_delegates_the_host_podman_command_not_a_shell(self):
        command = matrix.smoke_command(["flatpak-spawn", "--host", "podman"], "fixture", "image", Path("/repo"), Path("/repo/out"), "sha256:" + "a" * 64)
        self.assertEqual(command[:3], ["flatpak-spawn", "--host", "systemd-run"])
        self.assertNotIn("-c", command)

    def test_unprivileged_host_uid_mapping_is_required(self):
        with tempfile.TemporaryDirectory() as directory:
            mappings = smoke.verify_isolation("0 100000 1000\n1000 1001 1\n1001 101000 64535\n", Path(directory))
            self.assertEqual(mappings[0], [0, 100000, 1000])
            self.assertEqual(json.loads(json.dumps(mappings)), mappings)
            self.assertEqual(smoke.verify_isolation("0 1000 1\n1 100000 65536\n", Path(directory))[0], [0, 1000, 1])
            for invalid in ("", "0 0 4294967295", "0 -1 1", "1 100000 1000", "0 100000", "not a map"):
                with self.subTest(mapping=invalid), self.assertRaises(common.PackagingError):
                    smoke.verify_isolation(invalid, Path(directory))

    def test_hardware_paths_are_rejected_without_opening_them(self):
        for name in ("input", "uinput", "snd", "kvm", "mem", "kmem", "port"):
            with self.subTest(device=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / name).touch()
                with self.assertRaisesRegex(common.PackagingError, "forbidden"):
                    smoke.verify_isolation("0 100000 65536", root)

    def test_block_and_nonstandard_character_device_metadata_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            for mode in (stat.S_IFBLK, stat.S_IFCHR):
                entry = SimpleNamespace(lstat=lambda: SimpleNamespace(st_mode=mode, st_rdev=0))
                with self.subTest(mode=mode), patch.object(Path, "rglob", return_value=[entry]), \
                        patch.object(smoke.os, "major", return_value=13, create=True), \
                        patch.object(smoke.os, "minor", return_value=64, create=True), \
                        self.assertRaises(common.PackagingError):
                    smoke.verify_isolation("0 100000 65536", Path(directory))

    def test_already_promoted_rootless_bundle_is_rejected_without_new_transaction(self):
        manifest = {"target": {"id": "ubuntu-24.04"}, "package": {"file": "fixture.deb"},
                    "validation": {"native_smoke": "passed", "native_smoke_evidence": {"executor": "old"},
                                   "software": {name: "passed" for name in SOFTWARE_GATES}, "elf": "passed", "python_imports": "passed"}}
        def fixture_bytes(path, maximum, owner=None):
            if str(path) == "/proc/1/comm":
                return b"systemd\n"
            if str(path) == "/proc/self/uid_map":
                return b"0 1 1000\n1000 0 1\n1001 1001 64535\n"
            return b"ID=ubuntu\nVERSION_ID=24.04\n"
        verify = smoke.verify_isolation
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(sys, "argv", ["smoke.py", "--bundle", directory, "--builder-uid", "1000"]), \
                patch.object(smoke.sys, "platform", "linux"), \
                patch.object(smoke.os, "geteuid", return_value=0, create=True), \
                patch.dict(os.environ, {"PH_CI_SMOKE": "1", "PH_CI_SMOKE_EXECUTOR": "rootless-podman", "PH_CI_SMOKE_IMAGE_ID": "sha256:" + "a" * 64, "PH_CI_SMOKE_HOST_UID_MAP": "0 100000 65536\n"}), \
                patch.dict(sys.modules, {"pwd": SimpleNamespace(getpwnam=lambda name: SimpleNamespace(pw_uid=1000))}), \
                patch.object(Path, "is_file", return_value=True), \
                patch.object(smoke, "regular_bytes", side_effect=fixture_bytes), \
                patch.object(smoke, "verify_isolation", side_effect=lambda mappings: verify(mappings, Path(directory))), \
                patch.object(smoke, "verify_bundle", return_value=(manifest, "fixture")), \
                patch.object(smoke, "target_for", return_value={"id": "ubuntu-24.04", "manager": "apt"}), \
                patch.object(smoke, "run", side_effect=common.PackagingError("transaction reached")) as executed:
            with self.assertRaisesRegex(common.PackagingError, "already"):
                smoke.main()
            executed.assert_not_called()
            self.assertFalse((Path(directory) / "manifest.json").exists())
            self.assertEqual(manifest["validation"]["native_smoke"], "passed")

    def test_rootless_smoke_blocks_invalid_inputs_before_transaction(self):
        for invalid in ("missing", "failed", "elf", "imports", "ownership"):
            validation = {"native_smoke": "not-run", "software": {name: "passed" for name in SOFTWARE_GATES}, "elf": "passed", "python_imports": "passed"}
            if invalid == "missing":
                del validation["software"]
            elif invalid == "failed":
                validation["software"]["worker_alsa_phase_safety"] = "failed"
            elif invalid == "elf":
                validation["elf"] = "not-run"
            elif invalid == "imports":
                validation["python_imports"] = "failed"
            manifest = {"target": {"id": "ubuntu-24.04"}, "package": {"file": "fixture.deb"}, "validation": validation}
            def fixture_bytes(path, maximum, owner=None):
                if str(path) == "/proc/1/comm":
                    return b"systemd\n"
                return b"ID=ubuntu\nVERSION_ID=24.04\n"
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as directory, \
                    patch.object(sys, "argv", ["smoke.py", "--bundle", directory, "--builder-uid", "1000"]), \
                    patch.object(smoke.sys, "platform", "linux"), patch.object(smoke.os, "geteuid", return_value=0, create=True), \
                    patch.dict(os.environ, {"PH_CI_SMOKE": "1", "PH_CI_SMOKE_EXECUTOR": "rootless-podman", "PH_CI_SMOKE_IMAGE_ID": "sha256:" + "a" * 64, "PH_CI_SMOKE_HOST_UID_MAP": "0 100000 65536\n"}), \
                    patch.dict(sys.modules, {"pwd": SimpleNamespace(getpwnam=lambda name: SimpleNamespace(pw_uid=1000))}), \
                    patch.object(Path, "is_file", return_value=True), patch.object(smoke, "regular_bytes", side_effect=fixture_bytes), \
                    patch.object(smoke, "verify_isolation", return_value=[[0, 100000, 65536]]), \
                    patch.object(smoke, "verify_bundle", return_value=(manifest, "a" * 64)), \
                    patch.object(smoke, "target_for", return_value={"id": "ubuntu-24.04", "manager": "apt"}), \
                    patch.object(smoke, "run", side_effect=common.PackagingError("TRANSACTION REACHED")) as executed:
                bundle = Path(directory).resolve()
                path_stat = Path.stat
                def fixture_stat(path, *args, **kwargs):
                    return SimpleNamespace(st_uid=1000) if path == bundle else path_stat(path, *args, **kwargs)
                error = "bundle owner" if invalid == "ownership" else "software|audit"
                with patch.object(Path, "stat", fixture_stat), self.assertRaisesRegex(common.PackagingError, error):
                    smoke.main()
                executed.assert_not_called()


class MatrixGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.packaging = self.repo / "packaging"
        self.packaging.mkdir()
        self.commands = []
        self.output = io.StringIO()
        self.provenance = "docker.io/library/ubuntu@sha256:" + "a" * 64
        self.versions = "v24.11.0\nrustc 1.91.0 (fixture)\ncargo 1.91.0 (fixture)\n"
        self.info = {"host": {"security": {"rootless": True}, "cgroupVersion": "v2", "cgroupManager": "systemd"}}
        self.manifest = {"validation": {"native_smoke": "not-run"}}

    def invoke(self, *arguments):
        def command(argv, **kwargs):
            self.commands.append(argv)
            output = ""
            if "info" in argv:
                output = json.dumps(self.info)
            elif "{{.State.Pid}}" in argv:
                output = "12345"
            elif "inspect" in argv:
                output = self.provenance if "{{index .RepoDigests 0}}" in argv else "sha256:" + "b" * 64
            elif "python3" in argv and "-c" in argv:
                output = "0 100000 1000\n1000 1001 1\n1001 101000 64535\n"
            elif "--entrypoint" in argv:
                output = self.versions
            return SimpleNamespace(stdout=output, stderr="", returncode=0)

        with patch.object(sys, "argv", ["matrix.py", "--target", "ubuntu-24.04", "--output", str(self.repo / "out"), *arguments]), \
                patch.object(matrix, "HERE", self.packaging), patch.object(matrix, "require_build_user"), \
                patch.object(matrix, "load_targets", return_value={"targets": [{"id": "ubuntu-24.04", "image": "docker.io/library/ubuntu:24.04"}]}), \
                patch.object(matrix.os, "geteuid", return_value=1001, create=True), \
                patch.object(matrix.os, "getegid", return_value=1001, create=True), \
                patch.object(matrix, "run", side_effect=command), \
                patch.object(matrix, "verify_bundle", return_value=(self.manifest, "fixture")), \
                contextlib.redirect_stdout(self.output), contextlib.redirect_stderr(io.StringIO()):
            matrix.main()

    def test_docker_native_smoke_fails_before_pull_build_or_output_creation(self):
        with self.assertRaisesRegex(common.PackagingError, "fail-closed"):
            self.invoke()
        self.assertEqual(self.commands, [])
        self.assertFalse((self.repo / "out").exists())

    def test_prepare_only_verifies_sdk_without_application_or_native_transactions(self):
        self.invoke("--prepare-only", "--host-engine")
        result = json.loads(self.output.getvalue())
        self.assertEqual(result["sdk"], "ready")
        self.assertEqual(result["native_smoke"], "not-run")
        self.assertEqual(result["builder_uid"], 1001)
        self.assertEqual(result["sdk_tag"], "peripheral-hub-sdk:ubuntu-24.04-" + "b" * 12)
        self.assertTrue(any("tag" in command and result["sdk_tag"] in command for command in self.commands))
        self.assertTrue(all(command[:3] == ["flatpak-spawn", "--host", "docker"] for command in self.commands))
        self.assertFalse(any("--output" in command or "exec" in command or "--detach" in command for command in self.commands))
        self.assertFalse((self.repo / "out").exists())

    def test_unverified_base_image_digest_is_rejected(self):
        self.provenance = "ubuntu:24.04"
        with self.assertRaisesRegex(common.PackagingError, "immutable"):
            self.invoke("--prepare-only")
        self.assertFalse(any("build" in command for command in self.commands))

    def test_docker_and_podman_image_ids_are_complete_content_addresses(self):
        for value in ("a" * 64, "sha256:" + "a" * 64):
            with self.subTest(identifier=value), patch.object(matrix, "run", return_value=SimpleNamespace(stdout=value)):
                self.assertEqual(matrix.inspect_image_id(["podman"], "fixture"), "sha256:" + "a" * 64)
        for value in ("ubuntu:24.04", "abc123", "sha256:" + "g" * 64):
            with self.subTest(identifier=value), patch.object(matrix, "run", return_value=SimpleNamespace(stdout=value)), self.assertRaises(common.PackagingError):
                matrix.inspect_image_id(["docker"], "fixture")

    def test_wrong_node_or_rust_version_cannot_start_build(self):
        for versions in ("v18.20.0\nrustc 1.91.0 (fixture)\ncargo 1.91.0 (fixture)\n",
                         "v24.11.0\nrustc 1.85.0 (fixture)\ncargo 1.85.0 (fixture)\n"):
            self.versions = versions
            with self.subTest(versions=versions), self.assertRaisesRegex(common.PackagingError, "toolchain"):
                self.invoke("--prepare-only")
        self.assertFalse(any("--output" in command for command in self.commands))

    def test_rootful_podman_is_not_a_native_smoke_runner(self):
        self.info["host"]["security"]["rootless"] = False
        with self.assertRaisesRegex(common.PackagingError, "rootless"):
            self.invoke("--engine", "podman")
        self.assertEqual(len(self.commands), 1)

    def test_missing_systemd_cgroup_v2_is_not_silently_downgraded(self):
        for field, value in (("cgroupVersion", "v1"), ("cgroupManager", "cgroupfs")):
            self.info["host"][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(common.PackagingError, "cgroup"):
                self.invoke("--engine", "podman")
            self.info["host"][field] = "v2" if field == "cgroupVersion" else "systemd"

    def test_rootless_build_maps_host_uid_to_builder_1000(self):
        self.invoke("--engine", "podman", "--skip-smoke")
        probe = next(command for command in self.commands if "--entrypoint" in command)
        self.assertEqual(values(probe, "--network"), ["none"])
        for flag in ("--userns", "--volume"):
            self.assertNotIn(flag, probe)
        build_command = next(command for command in self.commands if "build" in command)
        self.assertIn("BUILDER_UID=1000", build_command)
        self.assertIn("BUILDER_GID=1000", build_command)
        build_run = next(command for command in self.commands if "--output" in command)
        self.assertEqual(values(build_run, "--user"), ["1000:1000"])
        self.assertEqual(values(build_run, "--userns"), ["keep-id:uid=1000,gid=1000"])
        self.assertEqual(values(build_run, "--cap-drop"), ["ALL"])
        self.assertIn("not-run (quarantined)", self.output.getvalue())

    def test_zero_smoke_exit_without_manifest_evidence_is_not_success(self):
        with self.assertRaisesRegex(common.PackagingError, "evidence"):
            self.invoke("--engine", "podman")
        self.assertTrue(any("--force" in command and "rm" in command for command in self.commands))
        smoke_run = next(command for command in self.commands if "/workspace/packaging/ci/smoke.py" in command)
        self.assertTrue(values(smoke_run, "--env")[0].startswith("PH_CI_SMOKE_HOST_UID_MAP=0 100000"))
        self.assertTrue(any("python3" in command and "/proc/12345/uid_map" in command[-1] for command in self.commands))
        self.assertNotIn("native_smoke=passed", self.output.getvalue())


class ToolchainSafetyTests(unittest.TestCase):
    def prepare(self, marker="/run/.containerenv", node_checksums=None, rust_checksum=None):
        node, rust = b"Node archive fixture", b"rustup executable fixture"
        checksum = hashlib.sha256(node).hexdigest().encode() + b"  node-v24.11.0-linux-x64.tar.xz\n"

        def fetch(url, maximum):
            if url.endswith("SHASUMS256.txt"):
                return checksum if node_checksums is None else node_checksums
            if url.endswith(".tar.xz"):
                return node
            if url.endswith(".sha256"):
                return hashlib.sha256(rust).hexdigest().encode() if rust_checksum is None else rust_checksum
            return rust

        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory)
            with patch.object(sys, "argv", ["toolchain.py", "--prefix", directory]), \
                    patch.object(toolchain, "require_build_user"), \
                    patch.dict(os.environ, {"PH_CI_CONTAINER": "1"}), \
                    patch.object(toolchain.os, "geteuid", return_value=prefix.stat().st_uid, create=True), \
                    patch.object(Path, "is_file", lambda path: marker is not None and path == Path(marker)), \
                    patch.object(toolchain, "https_bytes", side_effect=fetch) as fetched, \
                    patch.object(toolchain, "extract_node"), patch.object(toolchain.shutil, "move"), \
                    patch.object(toolchain, "run") as executed:
                toolchain.main()
                return executed.call_args, fetched.call_args_list

    def test_docker_and_podman_markers_support_pinned_nonroot_toolchains(self):
        for marker in ("/.dockerenv", "/run/.containerenv", "/.containerenv"):
            with self.subTest(marker=marker):
                call, fetched = self.prepare(marker)
                self.assertIn("-y", call.args[0])
                self.assertNotIn("--yes", call.args[0])
                self.assertIn("1.91.0", call.args[0])
                self.assertEqual(values(call.args[0], "--component"), ["rustfmt"])
                self.assertTrue(any("/v24.11.0/" in item.args[0] for item in fetched))
                self.assertTrue(any("/rustup/archive/1.28.2/" in item.args[0] for item in fetched))

    def test_no_container_marker_rejects_preparation(self):
        with self.assertRaisesRegex(common.PackagingError, "containers"):
            self.prepare(marker=None)

    def test_missing_or_duplicate_node_checksum_is_rejected(self):
        duplicate = (b"a" * 64 + b"  node-v24.11.0-linux-x64.tar.xz\n") * 2
        for invalid in (b"", duplicate, b"invalid  node-v24.11.0-linux-x64.tar.xz\n"):
            with self.subTest(checksums=invalid), self.assertRaisesRegex(common.PackagingError, "checksum"):
                self.prepare(node_checksums=invalid)

    def test_empty_or_mismatched_rustup_checksum_is_rejected(self):
        for invalid in (b"", b"0" * 64, b"not-a-digest"):
            with self.subTest(checksum=invalid), self.assertRaisesRegex(common.PackagingError, "checksum"):
                self.prepare(rust_checksum=invalid)


class SoftwareBuildGateTests(unittest.TestCase):
    def test_shared_native_preconditions_are_cross_target_and_keep_alsa_phase_gate(self):
        for target in common.load_targets()["targets"]:
            manifest = {"target": {"id": target["id"]}, "validation": {"software": {name: "passed" for name in SOFTWARE_GATES}, "elf": "passed", "python_imports": "passed", "native_smoke": "not-run"}}
            with self.subTest(target=target["id"]):
                gates.require_native_preconditions(manifest)
                manifest["validation"]["software"]["worker_alsa_phase_safety"] = "failed"
                with self.assertRaisesRegex(common.PackagingError, "software gate"):
                    gates.require_native_preconditions(manifest)
    def test_software_gate_runs_all_locked_suites_against_an_isolated_engine_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / "gate"
            with patch.object(gates, "require_build_user"), patch.object(gates, "run") as executed:
                evidence = gates.software_gate(work, Path(directory) / "cargo")
            commands = [call.args[0] for call in executed.call_args_list]
            for command in (["npm", "ci"], ["npm", "test"], ["npm", "run", "build"]):
                self.assertIn(command, commands)
            cargo = [command for command in commands if command[0] == "cargo"]
            self.assertEqual(len(cargo), 6)
            self.assertTrue(any("audio_worker::tests::lua_full_level_phase_safety" in command for command in cargo))
            self.assertTrue(all("--locked" in command for command in cargo))
            self.assertTrue(any(command[1] == "test" and "--no-default-features" not in command for command in cargo))
            self.assertTrue(any(command[1] == "test" and "--no-default-features" in command for command in cargo))
            for binary in gates.HELPER_BINARIES:
                self.assertIn(binary, cargo[0])
            self.assertTrue(any(str(work / "remapper-gate/tests") in command for command in commands))
            self.assertTrue(any(str(work / "remapper-gate/tests/linux") in command for command in commands))
            self.assertFalse((work / "remapper-gate/.cache").exists())
            self.assertEqual(evidence["pinned_engine_regressions"], "passed")
            self.assertEqual(evidence["worker_isolated_pipewire"], "passed")
            worker_call = next(call for call in executed.call_args_list if "isolated_pipewire_proxy_recycling_cubic_default_and_sofa" in call.args[0])
            self.assertEqual(worker_call.kwargs["env"]["PERIPHERAL_HUB_TEST_ISOLATED_PIPEWIRE"], "1")
            engine_call = executed.call_args_list[-1]
            self.assertEqual(engine_call.kwargs["env"]["PH_TEST_ENGINE_ROOTFS"], str(work / "remapper-gate/rootfs"))
            self.assertNotIn("native_smoke", evidence)

    def test_js_failure_stops_before_rust_or_packaging(self):
        def execute(command, **kwargs):
            if command == ["npm", "test"]:
                raise common.PackagingError("JS regression")
        with tempfile.TemporaryDirectory() as directory, patch.object(gates, "require_build_user"), \
                patch.object(gates, "run", side_effect=execute) as executed:
            with self.assertRaisesRegex(common.PackagingError, "JS regression"):
                gates.software_gate(Path(directory) / "gate", Path(directory) / "cargo")
            self.assertFalse(any(call.args[0][0] == "cargo" for call in executed.call_args_list))

    def test_actual_engine_failure_is_not_replaced_with_an_import_or_parse_check(self):
        def execute(command, **kwargs):
            if any(Path(part).parts[-3:] == ("remapper-gate", "tests", "linux") for part in command):
                raise common.PackagingError("actual engine regression")
        with tempfile.TemporaryDirectory() as directory, patch.object(gates, "require_build_user"), \
                patch.object(gates, "run", side_effect=execute):
            with self.assertRaisesRegex(common.PackagingError, "actual engine regression"):
                gates.software_gate(Path(directory) / "gate", Path(directory) / "cargo")

    def test_audio_worker_is_mandatory_elf_and_must_not_link_desktop_libraries(self):
        reports = {"usr/bin/" + binary: {"needed": ["libc.so.6"]} for binary in gates.HELPER_BINARIES}
        with patch.object(build, "audit", return_value=reports):
            self.assertEqual(build.audit_binary_payload(Path("/fixture")), reports)
            reports["usr/bin/peripheral-hub-audio-worker"]["needed"] = ["libwebkit2gtk-4.1.so.0"]
            with self.assertRaisesRegex(common.PackagingError, "desktop"):
                build.audit_binary_payload(Path("/fixture"))
            del reports["usr/bin/peripheral-hub-audio-worker"]
            with self.assertRaisesRegex(common.PackagingError, "Mandatory"):
                build.audit_binary_payload(Path("/fixture"))

    def test_pop_compatibility_does_not_authorize_a_fake_ubuntu_build(self):
        release = b"ID=pop\nVERSION_ID=24.04\nUBUNTU_CODENAME=noble\n"
        target = {"id": "ubuntu-24.04", "os_id": "ubuntu"}
        with patch.object(build, "require_build_user"), patch.object(build, "app_version", return_value="0.1.0"), \
                patch.object(build, "regular_bytes", return_value=release), \
                patch.object(build, "target_for", return_value=target) as selected, patch.object(build, "run") as executed:
            with self.assertRaisesRegex(common.PackagingError, "own distro"):
                build.build(SimpleNamespace(target="ubuntu-24.04"))
            self.assertFalse(selected.call_args.kwargs["allow_compatible"])
            executed.assert_not_called()

    def test_tauri_helper_hooks_use_locked_headless_builds(self):
        npm = common.json_file(PACKAGING.parent / "package.json")
        tauri = common.json_file(PACKAGING.parent / "src-tauri/tauri.conf.json")
        helper = npm["scripts"]["native:helpers"]
        self.assertIn("--locked", helper)
        self.assertIn("--no-default-features", helper)
        for binary in gates.HELPER_BINARIES:
            self.assertIn("--bin " + binary, helper)
        self.assertEqual(tauri["build"]["beforeDevCommand"], "npm run tauri:dev")
        self.assertEqual(tauri["build"]["beforeBuildCommand"], "npm run tauri:build")
        self.assertIn("stage_sidecars.py debug", npm["scripts"]["tauri:dev"])
        self.assertIn("stage_sidecars.py release", npm["scripts"]["tauri:build"])
        self.assertSetEqual(set(tauri["bundle"]["externalBin"]), {"target/tauri-sidecars/" + b for b in gates.HELPER_BINARIES})
        source = (PACKAGING / "build.py").read_text()
        self.assertIn('"--", "--locked"', source)
        self.assertIn('"audio_worker": {"binary": "usr/bin/peripheral-hub-audio-worker"', source)
        self.assertIn('installer / "build-target.json"', source)

    def test_tauri_sidecars_follow_current_profile_and_custom_cargo_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cargo = root / "custom-cargo-target"
            (root / "tauri.conf.json").write_bytes((PACKAGING.parent / "src-tauri/tauri.conf.json").read_bytes())
            for profile in ("debug", "release"):
                (cargo / profile).mkdir(parents=True)
                for name in gates.HELPER_BINARIES:
                    (cargo / profile / name).write_bytes(f"{profile}/{name}".encode())
                stage_sidecars.stage(cargo, root, profile, "x86_64-unknown-linux-gnu")
                for name in gates.HELPER_BINARIES:
                    actual = root / "target/tauri-sidecars" / f"{name}-x86_64-unknown-linux-gnu"
                    self.assertEqual(actual.read_bytes(), (cargo / profile / name).read_bytes())


class SDKDependencyTests(unittest.TestCase):
    def invoke(self, runtime_only, target_id="ubuntu-24.04", manager="apt"):
        target = {"id": target_id, "manager": manager, "build": ["build-essential"],
                  "runtime": ["python3"], "headset": ["libhidapi-hidraw0"], "audio": ["wireplumber"]}
        def resolve(target, packages):
            return [{"name": name} for name in packages], []
        with patch.object(sys, "argv", ["dependencies.py", "--target", target["id"], *(["--runtime-only"] if runtime_only else [])]), \
                patch.object(dependencies.sys, "platform", "linux"), \
                patch.object(dependencies.os, "geteuid", return_value=0, create=True), \
                patch.dict(os.environ, {"PH_CI_CONTAINER": "1"}), \
                patch.object(Path, "is_file", lambda path: path == Path("/run/.containerenv")), \
                patch.object(dependencies, "load_targets", return_value={"targets": [target]}), \
                patch.object(dependencies, "resolve_packages", side_effect=resolve), \
                patch.object(dependencies, "run") as executed:
            dependencies.main()
            return executed.call_args.args[0]

    def test_sdk_audio_and_lua_packages_are_container_build_dependencies(self):
        command = self.invoke(False)
        for name in ("libpipewire-0.3-dev", "libmysofa-dev", "lua5.4", "wireplumber"):
            self.assertIn(name, command)

    def test_native_smoke_image_does_not_inherit_sdk_dependencies(self):
        command = self.invoke(True)
        self.assertEqual(command, ["apt-get", "install", "--yes", "--no-install-recommends", "python3"])

    def test_fedora_smoke_image_installs_runuser_provider(self):
        self.assertEqual(self.invoke(True, "fedora-44", "dnf"),
                         ["dnf", "install", "--assumeyes", "python3", "util-linux"])
        self.assertNotIn("util-linux", self.invoke(False, "fedora-44", "dnf"))


class UbuntuSmokePolicyTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "policy-rc.d is a Linux shell script")
    def test_stock_policy_allows_only_package_service(self):
        with tempfile.TemporaryDirectory() as directory:
            policy = Path(directory) / "policy-rc.d"
            policy.write_bytes(b"#!/bin/sh\nexit 101\n")
            with contextlib.redirect_stdout(io.StringIO()):
                dependencies.allow_ubuntu_smoke_autostart(policy)
            for unit, expected in ((common.ENGINE, 0), ("systemd-udevd.service", 101),
                                   (common.ENGINE + ".other", 101)):
                with self.subTest(unit=unit):
                    result = subprocess.run(["sh", str(policy), unit, "start"], check=False)
                    self.assertEqual(result.returncode, expected)

    def test_custom_and_missing_policies_are_left_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            policy = Path(directory) / "policy-rc.d"
            dependencies.allow_ubuntu_smoke_autostart(policy)
            self.assertFalse(policy.exists())
            original = b"#!/bin/sh\n# Site policy\nexit 101\n"
            policy.write_bytes(original)
            dependencies.allow_ubuntu_smoke_autostart(policy)
            self.assertEqual(policy.read_bytes(), original)


class WorkflowGateTests(unittest.TestCase):
    def test_upload_requires_rootless_native_smoke_and_fixture_job_success(self):
        source = (PACKAGING.parent / ".github/workflows/linux-packaging.yml").read_text()
        self.assertIn("needs: installer-fixtures", source)
        self.assertIn("--engine podman", source)
        self.assertIn("if: success()", source)
        self.assertIn("packaging/out/quarantine/", source)
        self.assertNotIn("--skip-smoke", source)
        self.assertNotIn("--privileged", source)


if __name__ == "__main__":
    unittest.main()
