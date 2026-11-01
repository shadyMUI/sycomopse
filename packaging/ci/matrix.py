#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Blocking Docker build orchestration; artifacts only, no publishing or host services."""

import argparse
import os
import platform
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import HERE, PackagingError, load_targets, require_build_user, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=[item["id"] for item in load_targets()["targets"]])
    parser.add_argument("--output", type=Path, default=HERE / "out")
    parser.add_argument("--skip-smoke", action="store_true", help="Leave artifacts install-gated (native_smoke not-run)")
    args = parser.parse_args()
    require_build_user()
    target = next(item for item in load_targets()["targets"] if item["id"] == args.target)
    repo = HERE.parent.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    uid = os.geteuid()
    image = f"peripheral-hub-build:{target['id']}"
    run(["docker", "pull", "--platform", "linux/amd64", target["image"]], capture=False, timeout=600)
    provenance = run(["docker", "image", "inspect", "--format", "{{index .RepoDigests 0}}", target["image"]]).stdout.strip()
    match = re.search(r"@(sha256:[a-f0-9]{64})$", provenance)
    if not match:
        raise PackagingError("Docker did not return an immutable pulled image digest")
    run(["docker", "build", "--platform", "linux/amd64", "--file", str(HERE / "ci/Dockerfile"), "--build-arg", f"BASE_IMAGE={provenance}", "--build-arg", f"PH_TARGET={target['id']}", "--build-arg", f"BUILDER_UID={uid}", "--tag", image, str(repo)], capture=False, timeout=2400)
    run(["docker", "run", "--rm", "--platform", "linux/amd64", "--user", str(uid), "--volume", f"{repo}:/workspace", "--volume", f"{output}:/artifacts", "--env", f"PH_BUILD_IMAGE_DIGEST={match[1]}", image, "--target", target["id"], "--output", "/artifacts"], capture=False, timeout=5400)
    bundle = output / f"peripheral-hub-0.1.0-{target['id']}-x86_64"
    if not args.skip_smoke:
        # This container does not bind /dev/input, /usr/lib/modules or any host configuration.
        container = "peripheral-hub-smoke-" + target["id"] + "-" + str(os.getpid())
        try:
            run(["docker", "run", "--detach", "--name", container, "--platform", "linux/amd64", "--user", "0", "--privileged", "--cgroupns", "private", "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp", "--volume", f"{repo}:/workspace:ro", "--volume", f"{output}:/artifacts", "--env", "PH_CI_SMOKE=1", "--entrypoint", "/usr/lib/systemd/systemd", image], capture=False)
            run(["docker", "exec", container, "systemctl", "is-system-running", "--wait"], check=False, capture=False, timeout=120)
            run(["docker", "exec", container, "python3", "/workspace/packaging/ci/smoke.py", "--bundle", "/artifacts/" + bundle.name, "--builder-uid", str(uid)], capture=False, timeout=1800)
        finally:
            run(["docker", "rm", "--force", container], check=False, capture=False)
    print(f"Artifacts: {bundle}; no release/repository publication performed")


if __name__ == "__main__":
    main()
