#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Client/node/SOFA initialization against exact Noble and Pop servers, not the PCM DSP gate."""

import argparse
import io
import json
import os
import resource
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))
from audio_runtime import PREFIX
from common import PackagingError, digest, https_bytes, json_file, regular_bytes, run, safe_relative


def pop_server(root):
    root.mkdir()
    lock = json_file(PACKAGING / "ci/pipewire-server-test.lock.json")
    libs = "usr/lib/x86_64-linux-gnu/"
    modules = ("protocol-native", "client-node", "adapter", "spa-node-factory", "link-factory", "access", "metadata")
    selected = {"usr/bin/pipewire": "bin/pipewire", libs + "libpipewire-0.3.so.0.1608.0": "lib/libpipewire-0.3.so.0",
                **{libs + "pipewire-0.3/libpipewire-module-" + name + ".so": "lib/pipewire-0.3/libpipewire-module-" + name + ".so" for name in modules},
                **{libs + "spa-0.2/" + name + "/libspa-" + name + ".so": "lib/spa-0.2/" + name + "/libspa-" + name + ".so" for name in ("support", "audioconvert")}}
    found = set()
    for package in lock["packages"]:
        data = https_bytes(lock["base_url"] + package["file"], package["size"] + 1)
        if len(data) != package["size"] or digest(data) != package["sha256"] or not data.startswith(b"!<arch>\n"):
            raise PackagingError("Official Pop test package size/hash mismatch")
        offset, payload = 8, None
        while offset < len(data):
            header = data[offset:offset + 60]
            if len(header) != 60 or header[58:] != b"`\n":
                raise PackagingError("Invalid test DEB header")
            name, size = header[:16].decode("ascii").strip().rstrip("/"), int(header[48:58])
            offset += 60
            contents = data[offset:offset + size]
            if size < 0 or len(contents) != size:
                raise PackagingError("Truncated test DEB")
            offset += size + size % 2
            if name.startswith("data.tar"):
                if payload is not None:
                    raise PackagingError("Duplicate test DEB data")
                payload = contents
        if payload is None:
            raise PackagingError("Test DEB data missing")
        if payload.startswith(b"\x28\xb5\x2f\xfd"):
            # dpkg's native decompressor emits tar bytes only. Extraction still
            # uses the closed regular-file allowlist below, never tar -x.
            package_file = root / package["file"]
            package_file.write_bytes(data)
            unpacked = subprocess.run(["dpkg-deb", "--fsys-tarfile", str(package_file)], capture_output=True,
                                      timeout=12, preexec_fn=limits)
            package_file.unlink()
            if unpacked.returncode or len(unpacked.stdout) > 64 * 1024 * 1024:
                raise PackagingError("Bounded test DEB decompression failed")
            payload = unpacked.stdout
        total = 0
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
            for member in archive:
                name = member.name.removeprefix("./").rstrip("/")
                if name in ("", ".") and member.isdir():
                    continue
                safe_relative(name)
                if name not in selected:
                    continue
                total += member.size
                if not member.isfile() or name in found or member.size > 16 * 1024 * 1024 or total > 64 * 1024 * 1024:
                    raise PackagingError("Unsafe selected test ELF member")
                found.add(name)
                target = root / selected[name]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(archive.extractfile(member).read())
                target.chmod(0o755)
    if found != set(selected):
        raise PackagingError("Missing exact Pop test ELF files: " + str(set(selected) - found))
    for relative in selected.values():
        parent = Path(relative).parent
        rpath = "$ORIGIN/../lib" if parent == Path("bin") else "$ORIGIN" if parent == Path("lib") else "$ORIGIN:$ORIGIN/.." if parent == Path("lib/pipewire-0.3") else "$ORIGIN/../.."
        run(["patchelf", "--set-rpath", rpath, str(root / relative)])
    version = run([str(root / "bin/pipewire"), "--version"]).stdout
    if "Compiled with libpipewire 1.6.8" not in version or "Linked with libpipewire 1.6.8" not in version:
        raise PackagingError("Pop server executable/library not coherent: " + version)
    return root / "bin/pipewire", root / "lib/pipewire-0.3", root / "lib/spa-0.2"


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (512 * 1024 * 1024,) * 2)
    resource.setrlimit(resource.RLIMIT_CPU, (8, 8))


def check(root, server, server_modules, server_spa):
    root.mkdir()
    name = "ph-private-interop"
    base = {key: value for key, value in os.environ.items() if not key.startswith(("LD_", "PIPEWIRE_", "SPA_", "XDG_"))}
    base.update(XDG_RUNTIME_DIR=str(root), XDG_CONFIG_HOME=str(root), XDG_DATA_HOME=str(root), PIPEWIRE_RUNTIME_DIR=str(root),
                PIPEWIRE_CONFIG_DIR=str(root), PIPEWIRE_CONFIG_DATA_DIR=str(root), PIPEWIRE_CONFIG_PREFIX="", PIPEWIRE_REMOTE=name)
    server_env = dict(base, PIPEWIRE_MODULE_DIR=str(server_modules), SPA_PLUGIN_DIR=str(server_spa))
    client_env = dict(base, PIPEWIRE_MODULE_DIR=str(PREFIX / "lib/pipewire-0.3"), SPA_PLUGIN_DIR=str(PREFIX / "lib/spa-0.2"))
    config = {"context.properties": {"core.daemon": True, "core.name": name, "default.clock.rate": 48000},
              "context.spa-libs": {"audio.convert.*": "audioconvert/libspa-audioconvert", "support.*": "support/libspa-support"},
              "context.modules": [{"name": "libpipewire-module-" + module} for module in ("protocol-native", "client-node", "adapter", "spa-node-factory", "link-factory", "access", "metadata")],
              "context.objects": [{"factory": "spa-node-factory", "args": {"factory.name": "support.node.driver", "node.name": "interop-driver", "node.group": "pipewire.dummy", "priority.driver": 20000}}],
              "context.exec": []}
    (root / "server.conf").write_text(json.dumps(config))
    template = regular_bytes(PACKAGING.parent / "src-tauri/resources/spatializer-7.1.conf.template").decode()
    values = {"SESSION": "interop-sofa", "SOFA_PATH": "/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa", "INPUT_NAME": "interop-sofa-capture", "PLAYBACK_NAME": "interop-sofa-playback", "TARGET_OBJECT": "never-a-host-sink"}
    for key, value in values.items():
        template = template.replace("{{" + key + "}}", json.dumps(value))
    (root / "client.conf").write_text(template)
    version = subprocess.run([str(server), "--version"], env=server_env, capture_output=True, text=True,
                             timeout=2, preexec_fn=limits)
    if version.returncode:
        raise PackagingError("Server version failed: " + version.stderr)
    processes = []
    with (root / "server.log").open("wb") as server_log, (root / "client.log").open("wb") as client_log:
        try:
            processes.append(subprocess.Popen([str(server), "-c", "server.conf"], env=server_env, stdout=server_log, stderr=server_log, preexec_fn=limits))
            deadline = time.monotonic() + 6
            while not (root / name).exists() and time.monotonic() < deadline and processes[0].poll() is None:
                time.sleep(0.03)
            if not (root / name).exists():
                raise PackagingError("Isolated server failed: " + (root / "server.log").read_text()[-4000:])
            processes.append(subprocess.Popen([str(PREFIX / "bin/pipewire"), "-c", "client.conf"], env=client_env, stdout=client_log, stderr=client_log, preexec_fn=limits))
            observer = root / "observer"
            observer.mkdir()
            (observer / "client.conf").write_text(json.dumps({"context.properties": {"core.daemon": False},
                "context.spa-libs": config["context.spa-libs"],
                "context.modules": [{"name": "libpipewire-module-protocol-native"}, {"name": "libpipewire-module-client-node"}], "context.exec": []}))
            observer_env = dict(base, PIPEWIRE_CONFIG_DIR=str(observer), PIPEWIRE_CONFIG_DATA_DIR=str(observer),
                                PIPEWIRE_MODULE_DIR="/usr/lib/x86_64-linux-gnu/pipewire-0.3", SPA_PLUGIN_DIR="/usr/lib/x86_64-linux-gnu/spa-0.2")
            records = []
            while time.monotonic() < deadline and all(process.poll() is None for process in processes):
                observed = run(["pw-dump"], env=observer_env, timeout=2, check=False)
                if not observed.returncode:
                    objects = json.loads(observed.stdout)
                    records = [item for item in objects if item.get("type") == "PipeWire:Interface:Node" and item.get("info", {}).get("props", {}).get("node.name") in (values["INPUT_NAME"], values["PLAYBACK_NAME"])]
                    if len(records) == 2:
                        break
                time.sleep(0.03)
            if len(records) != 2:
                raise PackagingError("Private SOFA client failed to register: " + (root / "client.log").read_text()[-4000:])
            props = {item["info"]["props"]["node.name"]: item["info"]["props"] for item in records}
            if int(props[values["INPUT_NAME"]].get("audio.channels", 0)) != 8 or int(props[values["PLAYBACK_NAME"]].get("audio.channels", 0)) != 2:
                raise PackagingError("Registered client channels are not 8->2")
            # This verifies real SOFA graph initialization and native ClientNode
            # transport on the exact server, not impulse/PCM correctness.
            return {"client": "1.0.5-private", "server": version.stdout.strip(),
                    "node_registration": "passed", "channels": "8->2", "actual_dsp_gate": "not-run"}
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=0.2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pop", action="store_true", help="Fetch exact verified Pop binary packages into a temporary server root")
    args = parser.parse_args()
    status = regular_bytes("/proc/self/status", 32768).decode()
    if os.geteuid() != 1000 or not Path("/.dockerenv").is_file() or "CapEff:\t0000000000000000" not in status or "NoNewPrivs:\t1" not in status:
        raise PackagingError("Requires Docker UID1000/cap-drop ALL/no-new-privileges")
    with tempfile.TemporaryDirectory(prefix="audio-interop-") as directory:
        root = Path(directory)
        results = [check(root / "noble", Path("/usr/bin/pipewire"), Path("/usr/lib/x86_64-linux-gnu/pipewire-0.3"), Path("/usr/lib/x86_64-linux-gnu/spa-0.2"))]
        if args.pop:
            results.append(check(root / "pop", *pop_server(root / "pop-runtime")))
        print(json.dumps(results, sort_keys=True))


if __name__ == "__main__":
    main()
