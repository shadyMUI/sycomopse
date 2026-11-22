#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Private client packaging guards, not a replacement for the actual DSP gate."""

import io
import json
import os
import resource
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
import audio_runtime
import build
import common
import sources
import staging


def archive(mode, members):
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode=mode) as output:
        for name, kind in members:
            member = tarfile.TarInfo(name)
            member.type = kind
            if kind == tarfile.REGTYPE:
                member.size = 1
                output.addfile(member, io.BytesIO(b"x"))
            else:
                member.linkname = "../../outside"
                output.addfile(member)
    return data.getvalue()


class PrivatePipeWireTests(unittest.TestCase):
    def test_source_pins_are_the_official_noble_source_not_a_host_plugin(self):
        lock = common.json_file(PACKAGING / "pipewire.lock.json")
        self.assertEqual((lock["version"], lock["patchset"], lock["native_protocol"]), ("1.0.5", "1ubuntu3.3", "0.3"))
        self.assertEqual(lock["source"]["sha256"], "d5e5f3d0b8460e5711c1571c500156fd61768ba55a062eaeb94356abdb955a56")
        self.assertEqual(lock["debian"]["sha256"], "a1650f8e95859b3a8057ea95dad7e0509bfed9896db5b0d17c70910c4e9a3ff0")
        for name in ("source", "debian"):
            self.assertTrue(lock[name]["url"].startswith("https://archive.ubuntu.com/ubuntu/pool/main/p/pipewire/"))
        self.assertTrue(lock["checksum_source"].endswith("pipewire_1.0.5-1ubuntu3.3.dsc"))

    def test_build_includes_coherent_core_native_protocol_spa_and_sofa(self):
        files = audio_runtime.BUILD_FILES
        self.assertIn("src/daemon/pipewire", files)
        self.assertIn("src/pipewire/libpipewire-0.3.so.0.1005.0", files)
        for name in ("protocol-native", "client-node", "adapter", "filter-chain", "filter-chain-sofa"):
            self.assertIn("src/modules/libpipewire-module-" + name + ".so", files)
        self.assertIn("spa/plugins/support/libspa-support.so", files)
        self.assertIn("spa/plugins/audioconvert/libspa-audioconvert.so", files)
        self.assertNotIn("libpipewire-0.3.so.0", files.values())
        for relative in files.values():
            self.assertFalse(Path(relative).is_absolute())
            self.assertTrue(audio_runtime.rpath_for(relative).startswith("$ORIGIN"))

    def test_build_has_no_download_or_global_service_installation(self):
        options = audio_runtime.meson_options()
        for option in ("--wrap-mode=nodownload", "-Dlibmysofa=enabled", "-Dsession-managers=[]",
                       "-Dsystemd-user-service=disabled", "-Drlimits-install=false", "-Dflatpak=disabled"):
            self.assertIn(option, options)
        self.assertEqual(audio_runtime.PREFIX, Path("/usr/lib/peripheral-hub/pipewire"))

    def test_source_checksum_mismatch_cannot_reach_extract_or_compiler(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(sources, "https_bytes", return_value=b"not the pinned source"):
                with self.assertRaises(common.PackagingError):
                    sources.fetch_locked({"url": "https://example.invalid/source", "sha256": "a" * 64}, root, "source.tar.bz2")
            self.assertFalse((root / "source.tar.bz2").exists())

    def test_verified_source_compressions_extract_without_tar_extractall(self):
        for mode in ("w:gz", "w:bz2", "w:xz"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                destination = sources.extract_source(archive(mode, [("source/COPYING", tarfile.REGTYPE)]), directory)
                self.assertEqual((destination / "COPYING").read_bytes(), b"x")

    def test_colon_opt_in_preserves_real_posix_name_without_relaxing_bundle_paths(self):
        data = archive("w:bz2", [("source/samples/USB Device 0x46d:0x9a4", tarfile.REGTYPE)])
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(common.PackagingError):
                sources.extract_source(data, directory)
            destination = sources.extract_source(data, directory, allow_colons=True)
            self.assertEqual((destination / "samples/USB Device 0x46d:0x9a4").read_bytes(), b"x")
            with self.assertRaises(common.PackagingError):
                common.safe_relative("samples/USB Device 0x46d:0x9a4")

    def test_links_traversal_duplicates_and_multiple_roots_remain_rejected(self):
        candidates = [
            [("source/link", tarfile.SYMTYPE)], [("source/link", tarfile.LNKTYPE)],
            [("source/../../outside", tarfile.REGTYPE)], [("/source/outside", tarfile.REGTYPE)],
            [("source/a", tarfile.REGTYPE), ("source/a", tarfile.REGTYPE)],
            [("source/a", tarfile.REGTYPE), ("other/b", tarfile.REGTYPE)],
            [("source/a", tarfile.REGTYPE), ("source/a/b", tarfile.REGTYPE)],
        ]
        for members in candidates:
            with self.subTest(members=members), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises((common.PackagingError, OSError)):
                    sources.extract_source(archive("w:xz", members), directory, allow_colons=True)

    def test_native_deb_excludes_only_the_self_provided_core_and_keeps_real_sofa_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rootfs, work, destination = root / "rootfs", root / "work", root / "artifacts"
            for path in (rootfs, work, destination):
                path.mkdir()
            calls = []
            def run(command, **kwargs):
                calls.append(command)
                if command[0] == "dpkg-shlibdeps":
                    self.assertEqual((work / "debian/shlibs.local").read_text(), "libperipheral-hub-pipewire-0.3 0 peripheral-hub\n")
                    return SimpleNamespace(stdout="shlibs:Depends=libc6 (>= 2.38), libmysofa1 (>= 0.5), libsndfile1 (>= 1.0.20)")
                if "--build" in command:
                    self.assertIn("libsndfile1 (>= 1.0.20)", (rootfs / "DEBIAN/control").read_text())
                return SimpleNamespace(stdout="libc6 (>= 2.38), libmysofa1 (>= 0.5), libsndfile1 (>= 1.0.20)")
            with patch.object(build, "run", side_effect=run):
                _, dependencies = build.package_deb(rootfs, {"usr/lib/peripheral-hub/pipewire/bin/pipewire": {}}, [], work, destination, "0.1.0")
            self.assertIn("libmysofa1 (>= 0.5)", dependencies)
            command = calls[0]
            self.assertIn("-xperipheral-hub", command)
            self.assertFalse(any("ignore-missing" in argument for argument in command))
            self.assertIn("-l" + str(rootfs / "usr/lib/peripheral-hub/pipewire/lib"), command)


def native_checks():
    """Real loader/descriptor/ELF/dependency proof. Does not certify the DSP gate."""
    if os.geteuid() != 1000 or not Path("/.dockerenv").is_file():
        raise common.PackagingError("Run native ABI checks only in the nonprivileged SDK container")
    status = Path("/proc/self/status").read_text()
    if "CapEff:\t0000000000000000" not in status or "NoNewPrivs:\t1" not in status:
        raise common.PackagingError("Native ABI check requires cap-drop ALL and no-new-privileges")
    prefix = audio_runtime.PREFIX
    lock = common.json_file(PACKAGING / "pipewire.lock.json")
    metadata = json.loads(common.regular_bytes(prefix / "runtime.json", 32768, owner=0))
    if (metadata["generation"], metadata["sourceSha256"], metadata["patchsetSha256"]) != (
            lock["generation"], lock["source"]["sha256"], lock["debian"]["sha256"]):
        raise common.PackagingError("Private runtime provenance mismatch")
    for name, expected in metadata["files"].items():
        common.safe_relative(name)
        if common.digest(common.regular_bytes(prefix / name, 32 * 1024 * 1024, owner=0)) != expected:
            raise common.PackagingError("Private runtime file/hash mismatch: " + name)
    def limits():
        resource.setrlimit(resource.RLIMIT_AS, (512 * 1024 * 1024,) * 2)
        resource.setrlimit(resource.RLIMIT_CPU, (8, 8))
    with tempfile.TemporaryDirectory(prefix="private-audio-abi-") as directory:
        root = Path(directory)
        client = root / "client"
        shutil.copytree(prefix, client)
        for name in metadata["files"]:
            staging.elf_report(client / name)
        source = Path("/opt/peripheral-hub-audio/work/pipewire-source")
        cfile = root / "descriptor.c"
        cfile.write_text(r'''
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#include <mysofa.h>
#include "plugin.h"
int main(int argc, char **argv) {
    if (argc != 3) return 1;
    void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!lib) { fprintf(stderr, "%s\n", dlerror()); return 2; }
    fc_plugin_load_func *load = (fc_plugin_load_func *)dlsym(lib, FC_PLUGIN_LOAD_FUNC);
    if (!load) return 3;
    struct dsp_ops dsp = { 0 };
    struct fc_plugin *plugin = load(NULL, 0, &dsp, NULL, NULL);
    if (!plugin) return 4;
    const struct fc_descriptor *desc = plugin->make_desc(plugin, "spatializer");
    if (!desc || desc->n_ports != 6 || strcmp(desc->ports[0].name, "Out L") ||
        strcmp(desc->ports[1].name, "Out R") || strcmp(desc->ports[2].name, "In") ||
        strcmp(desc->ports[3].name, "Azimuth") || strcmp(desc->ports[4].name, "Elevation") ||
        strcmp(desc->ports[5].name, "Radius") || !desc->instantiate || !desc->run) return 5;
    fc_descriptor_free(desc);
    fc_plugin_free(plugin);
    dlclose(lib);
    int error = 0;
    struct MYSOFA_HRTF *hrtf = mysofa_load(argv[2], &error);
    if (!hrtf || error || mysofa_check(hrtf) || !hrtf->DataDelay.elements || !hrtf->DataDelay.values) return 6;
    for (unsigned int i = 0; i < hrtf->DataDelay.elements; i++)
        if (hrtf->DataDelay.values[i] != 0.0f) return 7;
    mysofa_free(hrtf);
    puts("private fc_plugin ABI 1.0.5: spatializer In -> Out L, Out R");
    return 0;
}
''', encoding="ascii")
        common.run(["cc", "-O2", "-I" + str(source / "spa/include"),
                    "-I" + str(source / "src/modules/module-filter-chain"), str(cfile), "-ldl", "-lmysofa", "-o", str(root / "descriptor")])
        descriptor = subprocess.run([str(root / "descriptor"), str(client / "lib/pipewire-0.3/libpipewire-module-filter-chain-sofa.so"),
                                     "/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa"],
                                    capture_output=True, text=True, timeout=12, preexec_fn=limits)
        if descriptor.returncode:
            raise common.PackagingError("Native SOFA descriptor failed: " + descriptor.stderr)
        debian = root / "debian"
        debian.mkdir()
        (debian / "control").write_text("Source: peripheral-hub\n\nPackage: peripheral-hub\nArchitecture: amd64\nDescription: temporary private audio dependency audit\n")
        (debian / "shlibs.local").write_text("libperipheral-hub-pipewire-0.3 0 peripheral-hub\n")
        dependencies = common.run(["dpkg-shlibdeps", "-O", "-xperipheral-hub", "-l" + str(client / "lib"),
                                   "-l" + str(client / "lib/pipewire-0.3"), *["-e" + str(client / name) for name in metadata["files"]]], cwd=root).stdout.strip()
        if "libmysofa1" not in dependencies or "libsndfile1" not in dependencies or "libperipheral-hub-pipewire" in dependencies:
            raise common.PackagingError("Incorrect real SOFA native dependency metadata: " + dependencies)
        (client / "lib" / audio_runtime.PRIVATE_LIB).unlink()
        missing = subprocess.run([str(client / "bin/pipewire"), "--version"], capture_output=True, text=True, timeout=12, preexec_fn=limits)
        if missing.returncode == 0 or audio_runtime.PRIVATE_LIB not in missing.stderr:
            raise common.PackagingError("Missing private core fell back to host PipeWire")
        print(json.dumps({"generation": metadata["generation"], "abi": "passed", "elf_count": len(metadata["files"]),
                          "descriptor": descriptor.stdout.strip(), "dependencies": dependencies, "missing_core": "rejected",
                          "default_sofa_delay": "verified-zero", "actual_dsp_gate": "not-run"}, sort_keys=True))


if __name__ == "__main__":
    if sys.argv[1:] == ["--native"]:
        native_checks()
    else:
        unittest.main()
