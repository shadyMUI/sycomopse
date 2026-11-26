#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Non-root software gates in the target SDK; no native installs or host devices."""

import argparse
import json
import os
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import HERE, PackagingError, require_build_user, run

HELPER_BINARIES = ("peripheral-hub-remapping-agent", "peripheral-hub-audio-worker")
SOFTWARE_GATES = ("packaging_tests", "npm_locked_install", "js_tests", "js_build", "headless_helpers_debug",
                  "rust_gui_tests", "rust_headless_tests", "worker_lua_contracts", "worker_alsa_phase_safety",
                  "worker_isolated_pipewire", "remapper_backend_validation_staging", "pinned_engine_regressions")


def require_native_preconditions(manifest):
    validation = manifest.get("validation", {})
    if not isinstance(validation, dict):
        raise PackagingError("Native smoke requires recorded software gates and staging audits")
    software = validation.get("software", {})
    if not isinstance(software, dict) or any(software.get(key) != "passed" for key in SOFTWARE_GATES):
        raise PackagingError("Native smoke requires every actual locked software gate to have passed")
    if validation.get("elf") != "passed" or validation.get("python_imports") != "passed":
        raise PackagingError("Native smoke requires successful binary/import staging audits")


def require_unpromoted(manifest):
    if manifest.get("validation", {}).get("native_smoke") == "passed":
        raise PackagingError("Native bundle already promoted; do not rerun or overwrite its successful evidence")


def software_gate(work, cargo_target, repo=HERE.parent):
    require_build_user()
    work, repo = Path(work), Path(repo)
    work.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_TARGET_DIR=str(Path(cargo_target).resolve()),
               PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(work.resolve()))
    manifest = str(repo / "src-tauri/Cargo.toml")
    helper_args = [argument for binary in HELPER_BINARIES for argument in ("--bin", binary)]
    commands = [
        ("packaging_tests", ["python3", "-B", "-m", "unittest", "discover", "-s", "packaging/tests", "-v"], 300),
        ("npm_locked_install", ["npm", "ci"], 600),
        ("js_tests", ["npm", "test"], 300),
        ("js_build", ["npm", "run", "build"], 600),
        ("headless_helpers_debug", ["cargo", "build", "--locked", "--manifest-path", manifest,
                                    "--no-default-features", *helper_args], 1200),
        ("rust_gui_tests", ["cargo", "test", "--locked", "--manifest-path", manifest], 1800),
        ("rust_headless_tests", ["cargo", "test", "--locked", "--manifest-path", manifest,
                                 "--no-default-features"], 1200),
        ("worker_lua_contracts", ["cargo", "test", "--locked", "--manifest-path", manifest,
                                  "--no-default-features", "--lib",
                                  "audio_worker::tests::lua_proxy_identity_and_readback_contracts", "--", "--ignored"], 600),
        ("worker_alsa_phase_safety", ["cargo", "test", "--locked", "--manifest-path", manifest,
                                      "--no-default-features", "--lib",
                                      "audio_worker::tests::lua_full_level_phase_safety", "--", "--ignored"], 600),
        ("worker_isolated_pipewire", ["cargo", "test", "--locked", "--manifest-path", manifest,
                                     "--no-default-features", "--test", "audio_worker",
                                     "isolated_pipewire_proxy_recycling_cubic_default_and_sofa", "--", "--ignored"], 600),
    ]
    evidence = {}
    for label, argv, timeout in commands:
        command_env = dict(env, PERIPHERAL_HUB_TEST_ISOLATED_PIPEWIRE="1") if label == "worker_isolated_pipewire" else env
        run(argv, cwd=repo, env=command_env, capture=False, timeout=timeout)
        if label == "headless_helpers_debug":
            # The next desktop cargo test runs tauri-build directly, without npm's beforeDevCommand.
            run([sys.executable, "-B", str(repo / "packaging/stage_sidecars.py"), "debug"],
                cwd=repo, env=env, capture=False, timeout=120)
        evidence[label] = "passed"

    # Preserve the tests' relative paths without writing vendor/cache into the source rootfs.
    remapper = work / "remapper-gate"
    shutil.copytree(repo / "packaging/remapper", remapper,
                    ignore=shutil.ignore_patterns(".cache", "__pycache__", "*.pyc", "vendor"))
    run(["python3", "-B", str(remapper / "fetch-engine.py"), "--destination", str(remapper / "rootfs")],
        cwd=repo, env=env, capture=False, timeout=180)
    env["PH_TEST_ENGINE_ROOTFS"] = str(remapper / "rootfs")
    run(["python3", "-B", "-m", "unittest", "discover", "-s", str(remapper / "tests"), "-v"],
        cwd=repo, env=env, capture=False, timeout=300)
    evidence["remapper_backend_validation_staging"] = "passed"
    engine_suite = remapper / "tests/linux/test_engine.py"
    if not engine_suite.is_file():
        raise PackagingError("The actual pinned-engine regression suite is required")
    run(["python3", "-B", "-m", "unittest", "discover", "-s", str(engine_suite.parent), "-v"],
        cwd=repo, env=env, capture=False, timeout=300)
    evidence["pinned_engine_regressions"] = "passed"
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workdir", required=True, type=Path)
    parser.add_argument("--cargo-target-dir", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps({"software": software_gate(args.workdir, args.cargo_target_dir),
                      "native_smoke": "not-run", "linux_hardware": "not-run"}, indent=2))


if __name__ == "__main__":
    main()
