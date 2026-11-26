"""Stage the freshly compiled native helpers for the matching Tauri build."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess


def stage(cargo_target, tauri_root, profile, triple):
    entries = json.loads((tauri_root / "tauri.conf.json").read_text())["bundle"]["externalBin"]
    for entry in entries:
        base = tauri_root / entry
        source = cargo_target / profile / base.name
        base.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, base.with_name(f"{base.name}-{triple}"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=("debug", "release"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1] / "src-tauri"
    metadata = subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1", "--manifest-path", str(root / "Cargo.toml")], text=True)
    version = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(row.removeprefix("host: ") for row in version.splitlines() if row.startswith("host: "))
    stage(Path(json.loads(metadata)["target_directory"]), root, args.profile, host)
