# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Fail-closed source selection and cleanup shared with the real engine adapter."""

from dataclasses import dataclass


STATES = frozenset(("UNKNOWN", "STARTING", "RUNNING", "STOPPED", "FAILED", "NO_GRAB", "UPGRADE_EVDEV"))


@dataclass(frozen=True)
class EngineStatus:
    state: str
    message: str | None = None
    verified: bool = False

    @property
    def active(self):
        return self.state in ("STARTING", "RUNNING")


class StrictFailure(Exception):
    def __init__(self, message, state="FAILED"):
        self.state = state
        super().__init__(message)


def strict_grab(sources_by_node, bindings, get_device_hash, verify_source=None):
    required = {}
    hashes = set()
    for binding in bindings:
        device = sources_by_node.get(binding.node_id)
        if device is None or str(get_device_hash(device)) != binding.origin_hash:
            raise StrictFailure("An expected source is missing or its originHash changed")
        if binding.code not in device.capabilities(absinfo=False).get(1, ()):
            raise StrictFailure("An expected source no longer emits the requested EV_KEY code")
        if binding.node_id not in required:
            if binding.origin_hash in hashes:
                raise StrictFailure("Expected source origin hashes are ambiguous")
            hashes.add(binding.origin_hash)
            required[binding.node_id] = device
    grabbed = {}
    try:
        for node_id, device in sorted(required.items()):
            if verify_source is not None:
                verify_source(node_id, device)
            device.grab()
            grabbed[node_id] = device
        for node_id, device in grabbed.items():
            if verify_source is not None:
                verify_source(node_id, device)
        # Do not replay queued pre-grab clicks or strand a key already held on
        # the original device. A short startup motion gap is safer than duplication.
        for device in grabbed.values():
            if device.active_keys():
                raise StrictFailure("Release source buttons before applying a private preset", "NO_GRAB")
            for _ in range(4096):
                if device.read_one() is None:
                    break
            else:
                raise StrictFailure("Source event queue did not become quiescent during startup", "NO_GRAB")
            if device.active_keys():
                raise StrictFailure("A source button became held during private startup", "NO_GRAB")
    except (OSError, StrictFailure) as error:
        for device in reversed(tuple(grabbed.values())):
            try:
                device.ungrab()
            except OSError:
                pass
        if isinstance(error, StrictFailure):
            raise
        raise StrictFailure("Exclusive source access failed; another remapper may own the device", "NO_GRAB") from None
    if not grabbed:
        raise StrictFailure("No effective source was selected", "NO_GRAB")
    return grabbed


class HeldKeys:
    """Only keys written to this process's private virtual output, never hardware state."""

    def __init__(self):
        self.codes = []

    def observe(self, type_, code, value):
        if type_ != 1:
            return
        if value == 1 and code not in self.codes:
            self.codes.append(code)
        elif value == 0 and code in self.codes:
            self.codes.remove(code)

    def release(self, write, syn):
        for code in reversed(self.codes.copy()):
            try:
                write(1, code, 0)
            except OSError:
                pass
        self.codes.clear()
        try:
            syn()
        except OSError:
            pass
