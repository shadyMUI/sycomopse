#!/usr/bin/env python3
"""Validate the worker from an unmodified source snapshot in an isolated OCI copy."""

import argparse
import os
import shutil
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--unit-only", action="store_true")
    args = parser.parse_args()
    if not (Path("/.dockerenv").exists() or Path("/run/.containerenv").exists()):
        raise SystemExit("The worker verification copy requires an isolated OCI container")
    source = Path(__file__).resolve().parents[1]
    work = args.workdir.resolve()
    if not work.is_relative_to(Path("/work")) or work == Path("/work") or work.exists():
        raise SystemExit("Use a new disposable directory below /work, never the source checkout")
    shutil.copytree(source, work, ignore=shutil.ignore_patterns("target", "__pycache__"))

    print("Worker verification uses an unmodified source copy", flush=True)

    env = dict(os.environ, CARGO_TARGET_DIR=str(work / "target"), PYTHONDONTWRITEBYTECODE="1")
    cargo = ["cargo", "--locked", "--manifest-path", str(work / "Cargo.toml"), "--no-default-features"]

    def run(arguments, timeout=900):
        subprocess.run(arguments, cwd=work, env=env, check=True, timeout=timeout)

    run([cargo[0], "test", *cargo[1:], "--lib", "audio_worker::tests"])
    if args.unit_only:
        print("Worker unit/process tests only; Lua, PipeWire and application integration NOT RUN", flush=True)
        return
    run([cargo[0], "test", *cargo[1:], "--lib",
         "audio_worker::tests::lua_proxy_identity_and_readback_contracts", "--", "--ignored"])
    run([cargo[0], "test", *cargo[1:], "--lib",
         "audio_worker::tests::lua_full_level_phase_safety", "--", "--ignored"])
    run([cargo[0], "test", *cargo[1:], "--lib",
         "audio_worker::tests::lua_guard_mutations_are_detected", "--", "--ignored"])
    run([cargo[0], "build", *cargo[1:], "--bin", "peripheral-hub-audio-worker"])
    dependencies = subprocess.run(
        ["ldd", str(work / "target/debug/peripheral-hub-audio-worker")],
        env=env, check=True, capture_output=True, text=True, timeout=30,
    ).stdout.lower()
    if any(gui in dependencies for gui in ("libgtk", "libwebkit", "libgdk", "libqt")):
        raise SystemExit("Headless audio worker unexpectedly links GUI libraries")
    print("Headless worker GUI-dependency gate passed", flush=True)
    env["PERIPHERAL_HUB_TEST_ISOLATED_PIPEWIRE"] = "1"
    run([cargo[0], "test", *cargo[1:], "--test", "audio_worker",
         "isolated_pipewire_proxy_recycling_cubic_default_and_sofa", "--", "--ignored"])
    print("Worker gates passed in the disposable copy. App integration/native packaging are separate gates.", flush=True)


if __name__ == "__main__":
    main()
