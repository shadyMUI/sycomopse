# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Load only the checksummed, private 2.2.1 engine, without its stock daemon."""

import hashlib
import importlib
import inspect
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys


VERSION = "2.2.1"
COMMIT = "e9a87d13480c3b1dee654b296578a8f4e2cd31d6"
ARCHIVE_SHA256 = "0bb5d167c59d32e917be90c48190a776ead2c855d5d239f0854ce7538ac97159"
PRIVATE = Path(__file__).resolve().parent


def trusted_path(path, require_root):
    metadata = path.lstat()
    if path.is_symlink() or not (stat.S_ISDIR(metadata.st_mode) or stat.S_ISREG(metadata.st_mode)):
        raise RuntimeError("Private engine paths must not contain links or special files")
    if require_root and (metadata.st_uid != 0 or metadata.st_mode & 0o022):
        raise RuntimeError("Private backend code must be root-owned and not group/world-writable")


def verify_vendor(directory=PRIVATE, require_root=True):
    directory = Path(directory)
    vendor = directory / "vendor"
    for path in (directory, vendor, vendor / "engine-lock.json"):
        trusted_path(path, require_root)
    lock = json.loads((vendor / "engine-lock.json").read_text(encoding="utf-8"))
    if (lock.get("version"), lock.get("commit"), lock.get("archiveSha256")) != (VERSION, COMMIT, ARCHIVE_SHA256):
        raise RuntimeError("The staged private engine does not match the frozen source lock")
    files = lock.get("files")
    if type(files) is not dict or not 1 <= len(files) <= 256:
        raise RuntimeError("Private engine manifest is invalid")
    for relative, expected in files.items():
        parts = PurePosixPath(relative).parts
        if not parts or parts[0] != "inputremapper" or any(part in (".", "..") or "\\" in part for part in parts) or not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise RuntimeError("Private engine manifest contains unsafe entries")
        path = vendor.joinpath(*parts)
        for parent in path.parents:
            if parent == vendor:
                break
            trusted_path(parent, require_root)
        trusted_path(path, require_root)
        if path.stat().st_size > 2 * 1024 * 1024 or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise RuntimeError("Private engine source checksum mismatch")
    paths = tuple((vendor / "inputremapper").rglob("*"))
    for path in paths:
        trusted_path(path, require_root)
    actual = {path.relative_to(vendor).as_posix() for path in paths if path.is_file()}
    if actual != set(files):
        raise RuntimeError("Private engine contains unverified files")
    return vendor


def load_engine(require_root=True):
    if sys.platform != "linux" or sys.version_info < (3, 10):
        raise RuntimeError("The private Input Remapper backend requires Linux and Python >= 3.10")
    if require_root:
        for path in (PRIVATE, *PRIVATE.parents):
            trusted_path(path, True)
        for path in PRIVATE.glob("*.py"):
            trusted_path(path, True)
    vendor = verify_vendor(require_root=require_root)
    existing = sys.modules.get("inputremapper")
    if existing is not None and Path(existing.__file__).resolve().parent != vendor / "inputremapper":
        raise RuntimeError("Refusing to reuse a stock or externally loaded Input Remapper engine")
    sys.path.insert(0, str(vendor))
    sys.dont_write_bytecode = True
    engine = importlib.import_module("inputremapper")
    if Path(engine.__file__).resolve().parent != vendor / "inputremapper":
        raise RuntimeError("Private engine import resolved outside the verified vendor directory")
    info = importlib.import_module("inputremapper.installation_info")
    info.COMMIT_HASH = COMMIT
    info.DATA_DIR = str(PRIVATE / "data")
    # GTK imports in the frozen gui package initializer are dependencies only;
    # no UI, reader service, stock config or stock daemon is instantiated.
    evdev = importlib.import_module("evdev")
    if not hasattr(evdev.InputDevice, "input_props") or "input_props" not in inspect.signature(evdev.UInput.__init__).parameters:
        raise RuntimeError("python-evdev must support InputDevice.input_props and UInput(input_props=...)")
    importlib.import_module("psutil")
    importlib.import_module("cairo")
    importlib.import_module("dasbus.server.interface")
    logger = importlib.import_module("inputremapper.logging.logger").logger
    logger.disabled = True
    paths = importlib.import_module("inputremapper.configs.paths").PathUtils

    def forbidden(*args, **kwargs):
        raise RuntimeError("Stock Input Remapper configuration APIs are disabled in the private backend")

    paths.config_path = staticmethod(lambda: "/run/peripheral-hub-input-remapper/no-stock-config")
    for name in ("get_config_path", "get_preset_path", "mkdir", "touch", "chown", "remove"):
        setattr(paths, name, staticmethod(forbidden))
    layout = importlib.import_module("inputremapper.configs.keyboard_layout").keyboard_layout
    symbols = {name: code for name, code in evdev.ecodes.ecodes.items() if re.fullmatch(r"KEY_[A-Z0-9_]+", name) and type(code) is int}
    layout._mapping = symbols
    layout._case_insensitive_mapping = {name.lower(): name for name in symbols}
    layout._xmodmap = []
    adapter = importlib.import_module("managed_engine")
    if "inputremapper.daemon" in sys.modules or "inputremapper.configs.global_config" in sys.modules:
        raise RuntimeError("The private engine must not import the stock daemon or configuration")
    return adapter.PrivateEngine(evdev)
