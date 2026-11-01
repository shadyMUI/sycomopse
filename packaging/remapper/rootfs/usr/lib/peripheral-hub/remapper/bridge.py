#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""The singleton root backend's restricted, versioned system-bus interface."""

import argparse
from contextlib import contextmanager
import json
import logging
import multiprocessing
import os
from pathlib import Path
import signal
import stat
import sys

# The installed wrapper uses Python -I. Only this trusted private directory is added.
PRIVATE = Path(__file__).resolve().parent
if __name__ == "__main__" and sys.platform == "linux" and os.geteuid() == 0 and "--check-imports" not in sys.argv:
    for path in (PRIVATE, *PRIVATE.parents, *PRIVATE.glob("*.py")):
        metadata = path.lstat()
        if path.is_symlink() or metadata.st_uid != 0 or metadata.st_mode & 0o022:
            raise SystemExit("Private backend code must be trusted before adding its import directory")
if str(PRIVATE) not in sys.path:
    sys.path.insert(0, str(PRIVATE))

from backend import Backend
from bootstrap import COMMIT, VERSION, load_engine
from credentials import Credentials
from inventory import LinuxDiscovery
from validation import BackendError, SecurePresetReader


BUS_NAME = "io.peripheralhub.InputRemapper"
OBJECT_PATH = "/io/peripheralhub/InputRemapper"
INTERFACE = "io.peripheralhub.InputRemapper1"
MAX_REPLY_BYTES = 1024 * 1024
XML = """<node>
  <interface name="io.peripheralhub.InputRemapper1">
    <method name="GetVersion"><arg direction="out" name="version" type="u"/></method>
    <method name="Inventory"><arg direction="out" name="inventory" type="s"/></method>
    <method name="State">
      <arg direction="in" name="fingerprint" type="s"/>
      <arg direction="out" name="state" type="s"/>
    </method>
    <method name="Apply">
      <arg direction="in" name="fingerprint" type="s"/>
      <arg direction="in" name="generation" type="t"/>
      <arg direction="in" name="preset" type="s"/>
      <arg direction="in" name="expectedPreset" type="s"/>
      <arg direction="in" name="configDir" type="s"/>
      <arg direction="out" name="state" type="s"/>
    </method>
    <method name="Stop">
      <arg direction="in" name="fingerprint" type="s"/>
      <arg direction="in" name="expectedPreset" type="s"/>
      <arg direction="out" name="state" type="s"/>
    </method>
  </interface>
</node>"""


def serialize(value):
    result = json.dumps(value, ensure_ascii=True, separators=(",", ":"), allow_nan=False)
    if len(result.encode("utf-8")) > MAX_REPLY_BYTES:
        raise BackendError("BackendFailure", "Private backend reply exceeds its bounded protocol size")
    return result


def make_interface(backend, accepts_additional_arguments):
    """The decorator is the real dasbus decorator at the production entry point."""
    class Interface:
        __dbus_xml__ = XML

        def GetVersion(self):
            return 1

        def Inventory(self):
            return serialize(backend.inventory())

        def State(self, fingerprint):
            return serialize(backend.state(fingerprint))

        @accepts_additional_arguments
        def Apply(self, fingerprint, generation, preset, expectedPreset, configDir, *, call_info):
            return serialize(backend.apply(fingerprint, generation, preset, expectedPreset, configDir, call_info))

        @accepts_additional_arguments
        def Stop(self, fingerprint, expectedPreset, *, call_info):
            return serialize(backend.stop(fingerprint, expectedPreset, call_info))

    return Interface()


class BusCredentials:
    """Bounded calls to the system bus daemon and logind, not client metadata."""

    def __init__(self, connection, glib, gio):
        self.connection = connection
        self.glib = glib
        self.gio = gio

    def _call(self, name, path, interface, method, signature, arguments):
        result = self.connection.call_sync(
            name, path, interface, method,
            self.glib.Variant(signature, arguments), None,
            self.gio.DBusCallFlags.NONE, 1500, None,
        )
        return result.unpack()

    def unix_user(self, sender):
        return self._call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixUser", "(s)", (sender,))[0]

    def sessions(self, uid):
        sessions = self._call("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "ListSessions", "()", ())[0]
        if len(sessions) > 256:
            raise RuntimeError("logind session inventory exceeds its bound")
        result = []
        for _, owner, _, seat, path in sessions:
            if owner != uid or seat != "seat0" or not path.startswith("/org/freedesktop/login1/session/"):
                continue
            properties = self._call("org.freedesktop.login1", path, "org.freedesktop.DBus.Properties", "GetAll", "(s)", ("org.freedesktop.login1.Session",))[0]
            user = properties.get("User", (None, None))
            session_seat = properties.get("Seat", (None, None))
            result.append({
                "uid": user[0], "path": path, "seat": session_seat[0],
                "type": properties.get("Type"), "class": properties.get("Class"),
                "state": properties.get("State"), "remote": properties.get("Remote"),
                "active": properties.get("Active"),
            })
        return result


@contextmanager
def singleton():
    import fcntl
    runtime = Path("/run/peripheral-hub-input-remapper")
    metadata = runtime.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != 0 or stat.S_IMODE(metadata.st_mode) != 0o700:
        raise RuntimeError("The private runtime directory must be root-owned, mode 0700, without symlinks")
    descriptor = os.open(runtime / "backend.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != 0 or stat.S_IMODE(metadata.st_mode) != 0o600 or metadata.st_nlink != 1:
            raise RuntimeError("The private singleton lock is unsafe")
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield descriptor
    finally:
        os.close(descriptor)


def serve(engine):
    import gi
    gi.require_version("GLib", "2.0")
    gi.require_version("Gio", "2.0")
    from gi.repository import GLib, Gio
    from dasbus.connection import SystemMessageBus
    from dasbus.server.handler import ServerObjectHandler
    from dasbus.server.interface import accepts_additional_arguments
    from inputremapper.utils import get_device_hash

    class PrivateServerHandler(ServerObjectHandler):
        def _handle_method_error(self, invocation, interface_name, method_name, error):
            code = error.code if isinstance(error, BackendError) else "BackendFailure"
            message = str(error) if isinstance(error, BackendError) else "Private backend operation failed"
            logging.warning("Private remapper request failed: %s", code)
            self._server.set_call_error(invocation, BUS_NAME + ".Error." + code, message)

    bus = SystemMessageBus()
    stream = bus.connection.get_stream()
    if not isinstance(stream, Gio.SocketConnection) or stream.get_socket().get_family() != Gio.SocketFamily.UNIX:
        raise RuntimeError("The private backend requires a local Unix system-bus connection")
    engine.inherited_fds = (*getattr(engine, "inherited_fds", ()), stream.get_socket().get_fd())
    lookups = BusCredentials(bus.connection, GLib, Gio)
    backend = Backend(LinuxDiscovery(engine.evdev, get_device_hash), engine, Credentials(lookups.unix_user, lookups.sessions), SecurePresetReader())
    loop = GLib.MainLoop()
    failed = False

    def maintain():
        nonlocal failed
        try:
            backend.maintain()
        except BackendError as error:
            logging.error("Private remapper maintenance failed: %s", error.code)
            failed = True
            loop.quit()
            return False
        return True

    def stop():
        loop.quit()
        return False

    def connection_closed(*args):
        nonlocal failed
        failed = True
        loop.quit()

    sources = []
    try:
        backend.inventory()
        bus.publish_object(OBJECT_PATH, make_interface(backend, accepts_additional_arguments), server_factory=PrivateServerHandler)
        # DO_NOT_QUEUE, with neither replacement nor allow-replacement flags.
        bus.register_service(BUS_NAME, flags=4)
        bus.connection.connect("closed", connection_closed)
        sources.append(GLib.timeout_add(1000, maintain))
        for signum in (signal.SIGTERM, signal.SIGINT):
            sources.append(GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signum, stop))
        loop.run()
    finally:
        for source in sources:
            GLib.source_remove(source)
        try:
            backend.shutdown()
        except BackendError:
            logging.error("Private injector shutdown could not be confirmed")
            failed = True
        bus.disconnect()
    return 1 if failed else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-imports", action="store_true", help="Verify staged engine/dependencies only; no DBus ownership or device grabs")
    args = parser.parse_args()
    logging.basicConfig(level=logging.WARNING, format="%(levelname)s: %(message)s")
    if args.check_imports:
        try:
            engine = load_engine(require_root=False)
            print(serialize({"engineVersion": VERSION, "engineCommit": COMMIT, "protocolVersion": 1, "keyboardOutputCodes": len(engine.keyboard_codes), "mouseOutputCodesVerified": sorted(engine.mouse_codes)}))
            return 0
        except Exception as error:
            print("Private engine import check failed: " + str(error), file=sys.stderr)
            return 1
    if sys.platform != "linux" or os.geteuid() != 0 or not sys.flags.isolated:
        print("Use the installed isolated wrapper under the private root system unit on Linux", file=sys.stderr)
        return 1
    for variable in ("DISPLAY", "WAYLAND_DISPLAY", "SUDO_USER", "PKEXEC_UID", "PYTHONPATH", "DBUS_SYSTEM_BUS_ADDRESS", "DBUS_SESSION_BUS_ADDRESS"):
        os.environ.pop(variable, None)
    os.environ.update(USER="root", HOME="/run/peripheral-hub-input-remapper", XDG_CONFIG_HOME="/run/peripheral-hub-input-remapper")
    if multiprocessing.get_start_method(allow_none=True) is None:
        multiprocessing.set_start_method("fork")
    if multiprocessing.get_start_method() != "fork":
        raise RuntimeError("The pinned private injector requires the Linux fork process context")
    try:
        with singleton() as descriptor:
            engine = load_engine()
            engine.inherited_fds = (descriptor,)
            return serve(engine)
    except Exception as error:
        logging.error("Private remapper startup failed: %s", error)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
