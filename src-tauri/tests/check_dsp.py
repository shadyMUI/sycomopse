#!/usr/bin/env python3
"""Public-controller DSP contract in an inputless, unprivileged SDK98 OCI server.

Run from the checkout: python3 -B src-tauri/tests/check_dsp.py --host-engine
Only the disposable Cargo download container has bridge networking. The actual
build/test container is offline; neither container mounts host audio or sessions.
The null sinks are synthetic software fixtures, NOT physical/HID certification.
Their device.api=alsa is ONLY classifier-admission metadata on a NULL factory;
no ALSA monitor, hardware factory, device identity or /dev/snd is provided.
"""

import argparse
import ctypes
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import unittest
import uuid
from pathlib import Path


SDK = "sha256:98bffd5168643b1f2d9d74e9e572983061536f84bc79ac090a8ba21eb98a4af7"
NATIVE_TEST = "real_controller_spatializer_71_stereo_contract"
TARGET = "dsp-contract.synthetic-stereo"
OTHER = "dsp-contract.synthetic-other"
DEVICES = ("/dev/input", "/dev/uinput", "/dev/snd")


def sandbox_error(oci, uid, devices, capabilities, no_new_privileges):
    if not oci:
        return "A real OCI filesystem marker is mandatory; no environment bypass"
    if uid != 1000 or capabilities != 0 or not no_new_privileges:
        return "Require UID1000, cap-drop ALL and no-new-privileges"
    if any(devices):
        return "Input and sound devices must be absent"
    return None


def require_sandbox():
    status = dict(line.split(":", 1) for line in Path("/proc/self/status").read_text().splitlines()
                  if ":" in line)
    error = sandbox_error(
        Path("/.dockerenv").is_file() or Path("/run/.containerenv").is_file(),
        os.geteuid(), [Path(path).exists() for path in DEVICES],
        int(status["CapEff"].strip(), 16), status["NoNewPrivs"].strip() == "1",
    )
    if error:
        raise RuntimeError(error)
    if os.getpid() != 1:
        raise RuntimeError("The staging runner must be PID1 in a private OCI PID namespace")
    for forbidden in ("/run/host", "/run/dbus/system_bus_socket", "/var/run/docker.sock",
                      "/run/podman/podman.sock", "/run/user/1000"):
        if Path(forbidden).exists():
            raise RuntimeError(f"No host/session/service mount is allowed: {forbidden}")
    mounts = []
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        left, right = line.split(" - ", 1)
        fields = left.split()
        mounts.append((Path(fields[4]), set(fields[5].split(",")), right.split()[0]))
    source = Path(__file__).resolve().parents[2]
    containing = [mount for mount in mounts if source.is_relative_to(mount[0])]
    source_mount = max(containing, key=lambda mount: len(mount[0].parts))
    work_mount = next(mount for mount in mounts if mount[0] == Path("/work"))
    error = mount_error(source_mount[1], work_mount[1], work_mount[2])
    if error:
        raise RuntimeError(error)


def mount_error(source_options, work_options, work_type):
    if "ro" not in source_options:
        return "The canonical source mount must be read-only"
    if work_type != "tmpfs" or not {"rw", "nosuid", "nodev"}.issubset(work_options) or "noexec" in work_options:
        return "Use executable, nosuid/nodev tmpfs scratch at /work"
    return None


def disposable_path(path):
    path = Path(path)
    if not path.is_absolute() or not path.is_relative_to("/work") or path == Path("/work"):
        raise ValueError("Use a new disposable directory strictly below /work")
    if ".." in path.parts:
        raise ValueError("Disposable paths may not contain parent traversal")
    return path


def test_outcome(output, minimum=1):
    matches = re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", output,
    )
    if len(matches) != 1 or int(matches[0][0]) < minimum or matches[0][1:] != ("0", "0"):
        raise RuntimeError("Tests must really execute: >=1 passed, zero failed, zero ignored")
    return int(matches[0][0])


class RunnerGuardTests(unittest.TestCase):
    def test_header_needs_real_oci_marker_and_device_absence(self):
        self.assertIsNone(sandbox_error(True, 1000, [False] * 3, 0, True))
        self.assertIsNotNone(sandbox_error(False, 1000, [False] * 3, 0, True))
        for index in range(3):
            devices = [False] * 3
            devices[index] = True
            self.assertIsNotNone(sandbox_error(True, 1000, devices, 0, True))

    def test_no_privileged_or_host_runtime_fallback(self):
        for uid, caps, nnp in ((0, 0, True), (1000, 1, True), (1000, 0, False)):
            self.assertIsNotNone(sandbox_error(True, uid, [False] * 3, caps, nnp))
        for path in ("/work", "/run/user/1000", "relative", "/work/a/../b"):
            with self.assertRaises(ValueError):
                disposable_path(path)

    def test_ignored_and_zero_tests_never_count_as_pass(self):
        self.assertEqual(test_outcome("test result: ok. 1 passed; 0 failed; 0 ignored;"), 1)
        for output in ("", "test result: ok. 0 passed; 0 failed; 1 ignored;",
                       "test result: ok. 0 passed; 0 failed; 0 ignored;",
                       "test result: ok. 1 passed; 1 failed; 0 ignored;"):
            with self.assertRaises(RuntimeError):
                test_outcome(output)

    def test_actual_mount_contract_has_no_writable_source_or_host_scratch(self):
        safe = {"rw", "nosuid", "nodev"}
        self.assertIsNone(mount_error({"ro"}, safe, "tmpfs"))
        self.assertIsNotNone(mount_error({"rw"}, safe, "tmpfs"))
        self.assertIsNotNone(mount_error({"ro"}, safe | {"noexec"}, "tmpfs"))
        self.assertIsNotNone(mount_error({"ro"}, safe, "ext4"))


class OwnedProcesses:
    def __init__(self, directory, deadline):
        self.directory = directory
        self.deadline = deadline
        self.children = []

    def remaining(self, maximum):
        remaining = min(maximum, self.deadline - time.monotonic())
        if remaining <= 0:
            raise TimeoutError("DSP runner exceeded its finite deadline")
        return remaining

    def spawn(self, label, command, env):
        log = (self.directory / f"{label}.log").open("wb")
        try:
            child = subprocess.Popen(command, env=env, cwd=self.directory,
                                     stdin=subprocess.DEVNULL, stdout=log,
                                     stderr=log, start_new_session=True)
        finally:
            log.close()
        self.children.append((label, child))
        return child

    def run(self, label, command, env, timeout=180):
        child = self.spawn(label, command, env)
        code = child.wait(timeout=self.remaining(timeout))
        output = (self.directory / f"{label}.log").read_text(errors="replace")
        if label != "cargo-fetch" or code:
            print(output, end="", flush=True)
        else:
            print("Locked Cargo cache prepared; download-only OCI, no native probe executed", flush=True)
        if code:
            raise RuntimeError(f"{label} exited {code}: {' '.join(map(str, command))}")
        return output

    def wait_until(self, predicate, description, timeout=10):
        deadline = time.monotonic() + self.remaining(timeout)
        while time.monotonic() < deadline:
            for label, child in self.children:
                if label in ("dbus", "pipewire", "wireplumber") and child.poll() is not None:
                    raise RuntimeError(f"Owned {label} exited {child.returncode} before {description}")
            if predicate():
                return
            time.sleep(0.05)
        raise TimeoutError(f"Private server did not become ready: {description}")

    def cleanup(self):
        # Each process group was created here; never use names, persisted PIDs or pkill.
        for _, child in reversed(self.children):
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        until = time.monotonic() + 3
        for _, child in reversed(self.children):
            try:
                child.wait(timeout=max(0.01, until - time.monotonic()))
            except subprocess.TimeoutExpired:
                pass
        for _, child in reversed(self.children):
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=3)
        # Helpers may deliberately have their own process groups. PID1/subreaper
        # ownership, not their executable name or a recovery journal, authorizes cleanup.
        for pid, start in self.descendants().items():
            try:
                fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
                if fields[19] == start:
                    os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except FileNotFoundError:
                pass
        # The runner is PID1/subreaper. Reap only orphaned descendants of our groups.
        until = time.monotonic() + 3
        while time.monotonic() < until:
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                break
            if pid == 0:
                time.sleep(0.02)
        for label, child in self.children:
            if Path(f"/proc/{child.pid}").exists():
                raise RuntimeError(f"Owned {label} PID {child.pid} was not reaped")
        remaining = self.descendants()
        if remaining:
            raise RuntimeError(f"Owned descendants were not reaped: {sorted(remaining)}")
        print("CLEANUP PROOF: all owned DBus/PipeWire/WirePlumber/helper/DSP descendants reaped", flush=True)

    def descendants(self):
        facts = {}
        for path in Path("/proc").iterdir():
            if not path.name.isdecimal() or int(path.name) == os.getpid():
                continue
            try:
                fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
                facts[int(path.name)] = (int(fields[1]), fields[19])
            except (FileNotFoundError, ProcessLookupError):
                pass
        owned = {os.getpid()}
        while True:
            more = {pid for pid, (parent, _) in facts.items() if parent in owned}
            if more.issubset(owned):
                return {pid: start for pid, (_, start) in facts.items() if pid in owned}
            owned.update(more)


def private_environment(runtime, work):
    env = os.environ.copy()
    for key in tuple(env):
        if key.startswith(("PIPEWIRE_", "PW_", "WIREPLUMBER_", "DBUS_", "XDG_")):
            env.pop(key)
    env.update({key: str(runtime) for key in ("XDG_RUNTIME_DIR", "PIPEWIRE_RUNTIME_DIR", "PW_RUNTIME_DIR")})
    env.update({key: str(work / key.lower()) for key in
                ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME")})
    env.update(HOME=str(work / "home"), LC_ALL="C", LANG="C", PYTHONDONTWRITEBYTECODE="1",
               CARGO_TARGET_DIR=str(work / "target"), CARGO_BUILD_JOBS="2",
               PIPEWIRE_REMOTE="peripheral-hub-dsp-" + uuid.uuid4().hex,
               DBUS_SESSION_BUS_ADDRESS=f"unix:path={runtime}/bus",
               DBUS_SYSTEM_BUS_ADDRESS=f"unix:path={runtime}/bus")
    return env


def stage_private_server(processes, runtime, env):
    bus = runtime / "dbus.conf"
    bus.write_text(f'''<busconfig>
      <type>session</type><listen>unix:path={runtime}/bus</listen>
      <auth>EXTERNAL</auth><policy context="default">
      <allow user="*"/><allow own="*"/>
      <allow send_destination="*"/><allow receive_sender="*"/>
      </policy></busconfig>''')
    dbus = processes.spawn("dbus", ["/usr/bin/dbus-daemon", "--nofork", f"--config-file={bus}"], env)
    processes.wait_until(lambda: (runtime / "bus").is_socket(), "private DBus socket")

    config = {
        "context.properties": {"core.daemon": True, "core.name": env["PIPEWIRE_REMOTE"],
                               "support.dbus": False, "mem.warn-mlock": False,
                               "default.clock.rate": 48000},
        "context.spa-libs": {"support.*": "support/libspa-support",
                             "audio.convert.*": "audioconvert/libspa-audioconvert"},
        "context.modules": [{"name": "libpipewire-module-" + name} for name in
                            ("protocol-native", "metadata", "spa-node-factory", "client-node",
                             "client-device", "adapter", "link-factory", "session-manager")]
                           + [{"name": "libpipewire-module-access", "args": {"access.force": "unrestricted"}}],
        "context.objects": [{"factory": "spa-node-factory", "args": {
            "factory.name": "support.node.driver", "node.name": "dsp-contract.driver",
            "node.group": "pipewire.dummy", "priority.driver": 20000}}],
        "context.exec": [],
    }
    for name in (TARGET, OTHER):
        # The public controller requires known-physical metadata in addition to
        # node.virtual=false. This is a DECLARED SYNTHETIC fixture, not an ALSA
        # device: the factory below stays support.null-audio-sink. Never weaken
        # the production classifier or claim physical coverage from this gate.
        config["context.objects"].append({"factory": "adapter", "args": {
            "factory.name": "support.null-audio-sink", "node.name": name,
            "node.description": "Synthetic software DSP fixture; NOT physical certification",
            "media.class": "Audio/Sink", "node.virtual": False,
            "device.api": "alsa",
            "peripheral-hub.synthetic-test": True, "audio.channels": 2,
            "audio.position": ["FL", "FR"], "audio.rate": 48000,
            "adapter.auto-port-config": {"mode": "dsp", "monitor": True, "position": "preserve"},
        }})
    config_path = runtime / "pipewire.conf"
    # PipeWire1.0's in-place SPA parser needs separation between nested closing
    # containers. Pretty JSON supplies it without replacing the real parser.
    config_path.write_text(json.dumps(config, indent=2))
    server_env = dict(env, PIPEWIRE_CONFIG_DIR=str(runtime), PIPEWIRE_CONFIG_PREFIX="",
                      PIPEWIRE_CONFIG_NAME="pipewire.conf")
    pipewire = processes.spawn("pipewire", ["/usr/bin/pipewire", "-c", str(config_path)], server_env)
    processes.wait_until(lambda: (runtime / env["PIPEWIRE_REMOTE"]).is_socket(), "private PipeWire socket")

    # Software-only WP0.4 policy: real upstream modules/scripts, no ALSA, BlueZ,
    # video, portal or logind monitors, and no default host configuration imports.
    wp_config = {
        "context.properties": {"application.name": "DSP contract WirePlumber", "log.level": 2},
        "context.spa-libs": config["context.spa-libs"],
        "context.modules": [{"name": "libpipewire-module-" + name} for name in
                            ("protocol-native", "client-node", "client-device", "adapter", "metadata")],
        "wireplumber.components": [{"name": "libwireplumber-module-" + name, "type": "module"}
                                   for name in ("lua-scripting", "metadata", "default-nodes", "si-node",
                                                "si-audio-adapter", "si-standard-link", "si-audio-endpoint",
                                                "default-nodes-api", "mixer-api")]
                                  + [{"name": name, "type": "script/lua", "arguments": {}}
                                     for name in ("create-item.lua", "policy-node.lua")],
    }
    wp_path = runtime / "wireplumber.conf"
    wp_path.write_text(json.dumps(wp_config, indent=2))
    wp = processes.spawn("wireplumber", ["/usr/bin/wireplumber", "-c", str(wp_path)],
                         dict(env, WIREPLUMBER_CONFIG_DIR=str(runtime)))

    def dump():
        output = subprocess.run(["/usr/bin/pw-dump", "-N"], env=env, capture_output=True,
                                check=True, timeout=processes.remaining(3)).stdout
        return json.loads(output)

    def ready():
        graph = dump()
        nodes = [node for node in graph if node["type"] == "PipeWire:Interface:Node"
                 and node.get("info", {}).get("props", {}).get("node.name") in (TARGET, OTHER)]
        metadata = [item for item in graph if item["type"] == "PipeWire:Interface:Metadata"
                    and item.get("props", {}).get("metadata.name") == "default"]
        return len(nodes) == 2 and len(metadata) == 1 and all(
            sum(port["type"] == "PipeWire:Interface:Port"
                 and str(port.get("info", {}).get("props", {}).get("node.id")) == str(node["id"])
                 and port.get("info", {}).get("direction") == "input"
                 and port.get("info", {}).get("props", {}).get("port.direction") == "in"
                 for port in graph) == 2
            for node in nodes)

    try:
        processes.wait_until(ready, "WirePlumber policy, stereo fixture ports and unique default metadata")
    except (RuntimeError, TimeoutError):
        graph = dump()
        facts = [{"id": item["id"], "type": item["type"],
                  "props": item.get("info", {}).get("props", item.get("props", {})),
                  "direction": item.get("info", {}).get("direction")}
                 for item in graph if item["type"] in ("PipeWire:Interface:Node", "PipeWire:Interface:Port",
                                                       "PipeWire:Interface:Metadata")]
        print("Private runtime readiness facts: " + json.dumps(facts, indent=2), flush=True)
        raise
    initial = dump()
    other = next(node for node in initial if node.get("info", {}).get("props", {}).get("node.name") == OTHER)
    processes.run("fixture-default", ["/usr/bin/wpctl", "set-default", str(other["id"])], env, 5)
    core = next(node for node in dump() if node["type"] == "PipeWire:Interface:Core")
    stage = {"schemaVersion": 1, "synthetic": True, "runtime": str(runtime),
             "remote": env["PIPEWIRE_REMOTE"], "coreCookie": int(core["info"]["cookie"]),
             "runnerPid": os.getpid(), "dbusPid": dbus.pid,
             "pipewirePid": pipewire.pid, "wireplumberPid": wp.pid}
    stage_path = runtime / "dsp-contract.json"
    stage_path.write_text(json.dumps(stage))
    stage_path.chmod(0o600)
    env["PH_DSP_CONTRACT_RUNTIME"] = str(runtime)
    return dump


def run_oci(args):
    require_sandbox()
    work = disposable_path(disposable_path(args.workdir).resolve())
    if work.exists():
        raise RuntimeError("Disposable workdir must not already exist")
    source = Path(__file__).resolve().parents[1]
    def source_hash(root):
        digest = hashlib.sha256()
        for directory, directories, files in os.walk(root):
            directories[:] = sorted(name for name in directories if name not in ("target", "__pycache__"))
            for name in sorted(files):
                path = Path(directory) / name
                relative = path.relative_to(root)
                digest.update(str(relative).encode() + b"\0" + path.read_bytes())
        return digest.hexdigest()

    snapshot_hash = source_hash(source)
    shutil.copytree(source, work, ignore=shutil.ignore_patterns("target", "__pycache__"))
    if source_hash(source) != snapshot_hash or source_hash(work) != snapshot_hash:
        shutil.rmtree(work)
        raise RuntimeError("Source changed during capture; do not test a transient/mixed snapshot")
    runtime = work / "private-runtime"
    runtime.mkdir(mode=0o700)
    env = private_environment(runtime, work)
    processes = OwnedProcesses(work, time.monotonic() + args.timeout)
    library = ctypes.CDLL(None, use_errno=True)
    if library.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "Cannot own/reap orphaned probe descendants")
    cargo = ["--locked", "--manifest-path", str(work / "Cargo.toml")]
    passed = None
    try:
        if args.fetch_only:
            processes.run("cargo-fetch", ["cargo", "fetch", *cargo,
                                          "--target", "x86_64-unknown-linux-gnu"], env, args.timeout)
            return
        if any(line.split(":", 1)[0].strip() != "lo"
               for line in Path("/proc/net/dev").read_text().splitlines()[2:] if ":" in line):
            raise RuntimeError("The actual DSP build/test OCI must use --network none")
        print("Unmodified source copy; software-only public Controller DSP, NOT hardware/HID certification", flush=True)
        print(f"Source snapshot SHA256: {snapshot_hash}", flush=True)
        processes.run("helper-build", ["cargo", "build", *cargo, "--offline", "--no-default-features",
                                       "--bin", "peripheral-hub-audio-worker"], env, args.timeout)
        dependencies = processes.run("helper-elf", ["ldd", str(work / "target/debug/peripheral-hub-audio-worker")], env, 10)
        if any(gui in dependencies.lower() for gui in ("libgtk", "libwebkit", "libgdk", "libqt")):
            raise RuntimeError("The sibling audio helper must be headless")
        guards = processes.run("rust-guards", ["cargo", "test", *cargo, "--offline", "--no-default-features",
                                              "--test", "audio_dsp_contract", "--", "--nocapture"], env, args.timeout)
        # Normal Cargo must not run native audio without a staged server. Count
        # the two executed safety tests only, never the explicitly ignored probe.
        normal = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", guards)
        if normal != [("2", "0", "1")] or f"test {NATIVE_TEST} ... ignored" not in guards:
            raise RuntimeError("Normal Cargo must execute two guards and ignore the unstaged native probe")
        print("Normal Cargo: 2 safety tests executed, 1 native test ignored (NOT passed)", flush=True)
        binaries = [path for path in (work / "target/debug/deps").glob("audio_dsp_contract-*")
                    if path.is_file() and os.access(path, os.X_OK) and path.suffix != ".d"]
        if len(binaries) != 1:
            raise RuntimeError("Cannot identify the exact sibling Cargo integration-test binary")
        dependencies = processes.run("test-elf", ["ldd", str(binaries[0])], env, 10)
        if any(gui in dependencies.lower() for gui in ("libgtk", "libwebkit", "libgdk", "libqt")):
            raise RuntimeError("Public Controller probe unexpectedly links GUI libraries")
        dump = stage_private_server(processes, runtime, env)
        native = processes.run("dsp-native", ["cargo", "test", *cargo, "--offline", "--no-default-features",
                                             "--test", "audio_dsp_contract", NATIVE_TEST,
                                             "--", "--exact", "--ignored", "--nocapture", "--test-threads=1"], env, 180)
        passed = test_outcome(native)
        if any(node.get("info", {}).get("props", {}).get("node.name", "").startswith("peripheral-hub.surround.")
               for node in dump()):
            raise RuntimeError("Owned DSP graph leaked after the public controller probe")
    finally:
        try:
            processes.cleanup()
        finally:
            for label, _ in processes.children:
                if label in ("dbus", "pipewire", "wireplumber"):
                    log = work / f"{label}.log"
                    print(f"[{label} owned log]\n{log.read_text(errors='replace')}", flush=True)
            shutil.rmtree(work)
    if passed is not None:
        print(json.dumps({"dspContract": "passed", "nativeTestsExecuted": passed,
                          "sourceSnapshotSha256": snapshot_hash, "syntheticSoftwareOnly": True,
                          "ownedProcessesReaped": True, "hardware": "not-run",
                          "nativePackageInstallation": "separate-gate"}), flush=True)


def run_host(args):
    runtime = ["flatpak-spawn", "--host", "docker"]
    source = Path(__file__).resolve().parents[2]
    image = subprocess.run([*runtime, "image", "inspect", "--format", "{{.Id}}", SDK],
                           check=True, capture_output=True, text=True, timeout=30).stdout.strip()
    if image != SDK:
        raise RuntimeError("Use the already prepared immutable SDK98, never rebuild or substitute it")
    token = uuid.uuid4().hex
    volume = "peripheral-hub-dsp-cargo-" + token
    deadline = time.monotonic() + args.timeout
    created = False
    owner_label = "io.peripheralhub.dsp-owner"
    try:
        actual = subprocess.run([*runtime, "volume", "create", "--label", f"{owner_label}={token}", volume], check=True,
                                capture_output=True, text=True, timeout=30).stdout.strip()
        if actual != volume:
            raise RuntimeError("Docker did not create the uniquely owned Cargo cache")
        owner = subprocess.run([*runtime, "volume", "inspect", "--format",
                                '{{index .Labels "' + owner_label + '"}}', volume], check=True,
                               capture_output=True, text=True, timeout=15).stdout.strip()
        if owner != token:
            raise RuntimeError("Never adopt or remove another invocation's Docker cache")
        created = True
        for fetch in (True, False):
            name = "peripheral-hub-dsp-" + token + ("-fetch" if fetch else "-native")
            # Docker's PID namespace is private by default; --pid=private is
            # Podman syntax. PID1/owned ancestry is verified inside the OCI.
            command = [*runtime, "run", "--rm", "--name", name, "--label", f"{owner_label}={token}", "--user", "1000:1000",
                       "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                       "--ipc", "private", "--network", "bridge" if fetch else "none",
                       "--pids-limit", "256", "--memory", "4g",
                       "--mount", f"type=bind,source={source},target=/source,readonly",
                       "--mount", f"type=volume,source={volume},target=/home/builder" + ("" if fetch else ",readonly"),
                       "--tmpfs", "/work:rw,exec,nosuid,nodev,uid=1000,gid=1000,size=3g",
                       "--tmpfs", "/tmp:rw,nosuid,nodev,mode=1777,size=128m",
                       "--env", "CARGO_HOME=/home/builder/dsp-cargo", "--env", "PYTHONDONTWRITEBYTECODE=1",
                       "--entrypoint", "python3", SDK, "-B", "/source/src-tauri/tests/check_dsp.py",
                       "--workdir", "/work/dsp", "--timeout", str(args.timeout)]
            if fetch:
                command.append("--fetch-only")
            try:
                subprocess.run(command, check=True, timeout=max(1, deadline - time.monotonic()))
            finally:
                # The only forced removal targets the random name created by this invocation.
                owner = subprocess.run([*runtime, "inspect", "--format",
                                        '{{index .Config.Labels "' + owner_label + '"}}', name],
                                       capture_output=True, text=True, timeout=15)
                if owner.returncode == 0 and owner.stdout.strip() == token:
                    subprocess.run([*runtime, "rm", "--force", name], check=True,
                                   capture_output=True, timeout=15)
                elif owner.returncode == 0:
                    raise RuntimeError("Never stop/remove an unowned OCI container")
    finally:
        if created:
            subprocess.run([*runtime, "volume", "rm", volume], check=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host-engine", action="store_true", help="Launch the fixed SDK using flatpak-spawn --host Docker")
    parser.add_argument("--workdir", type=Path)
    parser.add_argument("--fetch-only", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--timeout", type=int, default=900)
    args = parser.parse_args()
    if not 60 <= args.timeout <= 1800:
        parser.error("Timeout must be finite, between 60 and 1800 seconds")
    if args.self_test:
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(RunnerGuardTests))
        if not result.wasSuccessful() or result.testsRun < 4:
            raise SystemExit(1)
        return
    if args.host_engine:
        if os.geteuid() == 0 or args.workdir or args.fetch_only:
            parser.error("Host launch is non-root; workdir/fetch-only are private OCI arguments")
        run_host(args)
    else:
        if not args.workdir:
            parser.error("Use --host-engine or supply a disposable OCI --workdir")
        run_oci(args)


if __name__ == "__main__":
    def interrupted(signum, _frame):
        raise RuntimeError(f"DSP runner interrupted by signal {signum}; cleaning owned children")

    for handled in (signal.SIGTERM, signal.SIGINT):
        signal.signal(handled, interrupted)
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr, flush=True)
        raise SystemExit(1)
