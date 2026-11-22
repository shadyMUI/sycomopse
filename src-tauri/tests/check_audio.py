#!/usr/bin/env python3
"""Run controller regression tests from a disposable, inputless SDK checkout."""

import argparse
import os
import re
import shutil
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--all-library-tests", action="store_true")
    args = parser.parse_args()
    if os.geteuid() == 0 or not (Path("/.dockerenv").exists() or Path("/run/.containerenv").exists()):
        raise SystemExit("Use a non-root isolated OCI SDK, not the host")
    if any(Path(path).exists() for path in ("/dev/input", "/dev/uinput", "/dev/snd")):
        raise SystemExit("The controller test SDK must not expose host input or sound devices")
    work = args.workdir.resolve()
    if not work.is_relative_to(Path("/work")) or work == Path("/work") or work.exists():
        raise SystemExit("Use a new disposable directory below /work")
    source = Path(__file__).resolve().parents[1]
    shutil.copytree(source, work, ignore=shutil.ignore_patterns("target", "__pycache__"))
    env = dict(os.environ, CARGO_TARGET_DIR=str(work / "target"), PYTHONDONTWRITEBYTECODE="1",
               CARGO_BUILD_JOBS="2")
    command = ["cargo", "test", "--locked", "--manifest-path", str(work / "Cargo.toml"),
               "--no-default-features", "--lib"]
    if not args.all_library_tests:
        command.append("audio::")
    command.extend(["--", "--test-threads=4"])
    result = subprocess.run(command, cwd=work, env=env, text=True, capture_output=True, timeout=1200)
    print(result.stdout, end="", flush=True)
    print(result.stderr, end="", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
    outcome = re.search(r"test result: ok\. (\d+) passed; 0 failed", result.stdout)
    if outcome is None or int(outcome[1]) < 19:
        raise SystemExit("The audio regression tests were not actually executed")
    print("Controller regression gate passed; native audio, installation and hardware remain separate gates", flush=True)


if __name__ == "__main__":
    main()
