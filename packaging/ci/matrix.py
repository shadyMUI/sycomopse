#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Unprivileged OCI builds; native smoke only in delegated rootless Podman."""

import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import HERE, PackagingError, load_targets, require_build_user, run, verify_bundle


def require_rootless_systemd(runtime):
    info = json.loads(run([*runtime, "info", "--format", "json"]).stdout)
    host = info.get("host", {}) if isinstance(info, dict) else {}
    security = host.get("security", {}) if isinstance(host, dict) else {}
    if not isinstance(security, dict) or security.get("rootless") is not True:
        raise PackagingError("Native smoke requires rootless Podman, never a rootful daemon")
    if host.get("cgroupVersion") != "v2" or host.get("cgroupManager") != "systemd":
        raise PackagingError("Native smoke requires cgroup v2 and Podman's systemd cgroup manager")


def inspect_image_id(runtime, image):
    value = run([*runtime, "image", "inspect", "--format", "{{.Id}}", image]).stdout.strip()
    match = re.fullmatch(r"(?:sha256:)?([a-f0-9]{64})", value)
    if not match:
        raise PackagingError("The OCI client did not identify the complete SDK/runtime image SHA256")
    return "sha256:" + match[1]


def smoke_command(runtime, container, image, repo, output, image_id):
    if runtime[-1] != "podman":
        raise PackagingError("Docker systemd smoke is refused; no privileged fallback is allowed")
    # SYS_ADMIN is confined to the rootless user namespace for systemd's private mounts.
    capabilities = ("CHOWN", "DAC_OVERRIDE", "DAC_READ_SEARCH", "FOWNER", "SETUID", "SETGID",
                    "SETPCAP", "KILL", "SYS_CHROOT", "SYS_ADMIN")
    # Rootless Podman rejects custom device rules. Use Podman's default private
    # /dev without host device passthrough; smoke.py verifies it before installing.
    # The default rootless UID map lets container root write the host's bundle
    # without a keep-id layer copy. The in-container session user stays non-root.
    return [*runtime[:-1], "systemd-run", "--user", "--scope", "--quiet", "--property=Delegate=yes", "--",
            runtime[-1], "run", "--detach", "--name", container, "--platform", "linux/amd64",
            "--user", "0:0", "--systemd", "always",
            "--cgroupns", "private", "--cgroups", "enabled", "--pid", "private", "--ipc", "private",
            "--network", "private", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            *[argument for capability in capabilities for argument in ("--cap-add", capability)],
            "--pids-limit", "512", "--memory", "2g",
            "--tmpfs", "/run:rw,nosuid,nodev,mode=755", "--tmpfs", "/run/lock:rw,nosuid,nodev",
            "--tmpfs", "/tmp:rw,nosuid,nodev,mode=1777", "--volume", f"{repo}:/workspace:ro",
            "--volume", f"{output}:/artifacts:rw", "--env", "PH_CI_SMOKE=1",
            "--env", "PH_CI_SMOKE_EXECUTOR=rootless-podman", "--env", f"PH_CI_SMOKE_IMAGE_ID={image_id}", image]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=[item["id"] for item in load_targets()["targets"]])
    parser.add_argument("--output", type=Path, default=HERE / "out/quarantine")
    parser.add_argument("--engine", choices=("docker", "podman"), default="docker")
    parser.add_argument("--host-engine", action="store_true", help="Access the OCI client through flatpak-spawn --host")
    parser.add_argument("--prepare-only", action="store_true", help="Prepare and verify the SDK image; do not build the application")
    parser.add_argument("--skip-smoke", action="store_true", help="Leave artifacts install-gated (native_smoke not-run)")
    args = parser.parse_args()
    require_build_user()
    if args.engine == "docker" and not (args.skip_smoke or args.prepare_only):
        raise PackagingError("Docker native smoke is fail-closed. Use rootless Podman with delegated systemd/cgroup v2, or --skip-smoke for quarantine only")
    runtime = (["flatpak-spawn", "--host"] if args.host_engine else []) + [args.engine]
    if args.engine == "podman":
        require_rootless_systemd(runtime)
    target = next(item for item in load_targets()["targets"] if item["id"] == args.target)
    repo = HERE.parent.resolve()
    output = args.output.resolve()
    uid, gid = (1000, 1000) if args.engine == "podman" else (os.geteuid(), os.getegid())
    image = f"peripheral-hub-build:{target['id']}"
    run([*runtime, "pull", "--platform", "linux/amd64", target["image"]], capture=False, timeout=600)
    provenance = run([*runtime, "image", "inspect", "--format", "{{index .RepoDigests 0}}", target["image"]]).stdout.strip()
    match = re.search(r"@(sha256:[a-f0-9]{64})$", provenance)
    if not match:
        raise PackagingError("The OCI client did not return an immutable pulled image digest")
    run([*runtime, "build", "--platform", "linux/amd64", "--file", str(HERE / "ci/Dockerfile"), "--build-arg", f"BASE_IMAGE={provenance}", "--build-arg", f"PH_TARGET={target['id']}", "--build-arg", f"BUILDER_UID={uid}", "--build-arg", f"BUILDER_GID={gid}", "--tag", image, str(repo)], capture=False, timeout=2400)
    image_id = inspect_image_id(runtime, image)
    sdk_tag = f"peripheral-hub-sdk:{target['id']}-{image_id.removeprefix('sha256:')[:12]}"
    run([*runtime, "tag", image, sdk_tag])
    print(f"Checking {target['id']} SDK toolchain versions ...", file=sys.stderr, flush=True)
    # No bind mounts here, so keep-id's potentially expensive layer copies are unnecessary.
    versions = run([*runtime, "run", "--rm", "--network", "none", "--user", f"{uid}:{gid}", "--cap-drop", "ALL",
                    "--security-opt", "no-new-privileges", "--entrypoint", "/bin/sh", image, "-c",
                    "node --version && rustc --version && cargo --version"]).stdout.strip().splitlines()
    if len(versions) != 3 or versions[0] != "v24.11.0" or not versions[1].startswith("rustc 1.91.0 ") or not versions[2].startswith("cargo 1.91.0 "):
        raise PackagingError("SDK toolchain versions do not match the verified Node24.11/Rust1.91 contract")
    if args.prepare_only:
        print(json.dumps({"sdk": "ready", "image": image, "sdk_tag": sdk_tag, "image_id": image_id,
                          "base_image": provenance, "builder_uid": uid, "builder_gid": gid,
                          "toolchain": versions, "native_smoke": "not-run"}, indent=2))
        return
    output.mkdir(parents=True, exist_ok=True)
    userns = ["--userns", "keep-id:uid=1000,gid=1000"] if args.engine == "podman" else []
    # Reap orphaned build children rather than accumulating zombies under Python PID 1.
    reaper = ["--init"] if target["id"] == "cachyos" else []
    preparation = " (Podman may first copy image layers)" if userns else ""
    print(f"Starting {target['id']} application build{preparation} ...", file=sys.stderr, flush=True)
    run([*runtime, "run", "--rm", *reaper, "--platform", "linux/amd64", *userns, "--user", f"{uid}:{gid}", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--volume", f"{repo}:/workspace:rw", "--volume", f"{output}:/artifacts:rw", "--env", f"PH_BUILD_IMAGE_DIGEST={match[1]}", image, "--target", target["id"], "--output", "/artifacts"], capture=False, timeout=7200)
    bundle = output / f"peripheral-hub-0.1.0-{target['id']}-x86_64"
    if not args.skip_smoke:
        smoke_image = "peripheral-hub-smoke:" + target["id"]
        run([*runtime, "build", "--platform", "linux/amd64", "--file", str(HERE / "ci/Smoke.Dockerfile"), "--build-arg", f"BASE_IMAGE={provenance}", "--build-arg", f"PH_TARGET={target['id']}", "--tag", smoke_image, str(repo)], capture=False, timeout=1800)
        smoke_id = inspect_image_id(runtime, smoke_image)
        container = "peripheral-hub-smoke-" + target["id"] + "-" + str(os.getpid())
        try:
            run(smoke_command(runtime, container, smoke_image, repo, output, smoke_id), capture=False)
            run([*runtime, "exec", container, "systemctl", "is-system-running", "--wait"], check=False, capture=False, timeout=120)
            pid = run([*runtime, "inspect", "--format", "{{.State.Pid}}", container]).stdout.strip()
            if not re.fullmatch(r"[1-9][0-9]*", pid):
                raise PackagingError("Cannot identify the managed smoke PID for host UID mapping verification")
            # Read only this container's proc metadata in the initial host UID view.
            reader = "from pathlib import Path; import sys; " \
                     "identity = Path('/proc/self/uid_map').read_text().split(); " \
                     "identity == ['0', '0', '4294967295'] or sys.exit('Initial host UID view required'); " \
                     f"print(Path('/proc/{pid}/uid_map').read_text(), end='')"
            host_map = run([*runtime[:-1], "python3", "-c", reader]).stdout
            run([*runtime, "exec", "--user", "0:0", "--env", "PH_CI_SMOKE_HOST_UID_MAP=" + host_map, container, "python3", "-B", "/workspace/packaging/ci/smoke.py", "--bundle", "/artifacts/" + bundle.name, "--builder-uid", str(uid)], capture=False, timeout=1800)
            manifest, _ = verify_bundle(bundle)
            if manifest["validation"].get("native_smoke") != "passed" or manifest["validation"].get("native_smoke_evidence", {}).get("executor") != "rootless-podman":
                raise PackagingError("Native smoke did not produce verified rootless package-lifecycle evidence")
        finally:
            run([*runtime, "rm", "--force", container], check=False, capture=False)
    print(f"Artifacts: {bundle}; native_smoke={'not-run (quarantined)' if args.skip_smoke else 'passed (package lifecycle only)'}; no release/repository publication performed")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("Build interrupted.", file=sys.stderr)
        sys.exit(130)
    except (PackagingError, OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
