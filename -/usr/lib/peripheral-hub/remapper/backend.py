# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Atomic managed state. Dependencies are explicit, including in the test suite."""

from dataclasses import dataclass
import secrets
import threading

from inventory import SUPPORTED_SEAT
from strict import EngineStatus, STATES
from validation import BackendError, FINGERPRINT, PresetValidator, integer, preset_identity


@dataclass
class Running:
    state: str = "STOPPED"
    message: str | None = None
    preset: str | None = None
    owner_uid: int | None = None
    session: object | None = None
    signature: str | None = None
    handle: object | None = None


class Backend:
    def __init__(self, discovery, engine, credentials, preset_reader, generation=None):
        self.discovery = discovery
        self.engine = engine
        self.credentials = credentials
        self.preset_reader = preset_reader
        self.validator = PresetValidator(engine.keyboard_codes, engine.mouse_codes)
        self.generation = generation if generation is not None else secrets.randbits(64) or 1
        if not integer(self.generation, 1, 2**64 - 1):
            raise ValueError("Invalid daemon generation")
        self.devices = {}
        self.topology = None
        self.running = {}
        self.lock = threading.RLock()
        self.closed = False

    @staticmethod
    def _fingerprint(fingerprint):
        if not isinstance(fingerprint, str) or not FINGERPRINT.fullmatch(fingerprint):
            raise BackendError("DeviceUnavailable", "Fingerprint must identify a discovered physical mouse")

    def _observe(self, record):
        if record.handle is None:
            return
        try:
            status = record.handle.status()
        except Exception:
            self._stop_record(record, "FAILED", "Injector status could not be verified")
            return
        if not isinstance(status, EngineStatus) or status.state not in STATES or (status.state == "RUNNING" and not status.verified):
            self._stop_record(record, "FAILED", "Injector did not confirm every expected physical source")
            return
        record.state = status.state
        record.message = status.message
        if not status.active:
            self._stop_record(record, status.state, status.message)

    def _stop_record(self, record, state="STOPPED", message=None):
        if record.handle is not None:
            try:
                status = record.handle.stop()
            except Exception:
                record.state = "FAILED"
                record.message = "Private injector shutdown could not be confirmed"
                raise BackendError("BackendFailure", "Private injector shutdown could not be confirmed") from None
            if not isinstance(status, EngineStatus) or status.state != "STOPPED":
                record.state = "FAILED"
                record.message = "Private injector shutdown did not confirm termination"
                raise BackendError("BackendFailure", "Private injector shutdown did not confirm termination")
        record.handle = None
        record.preset = None
        record.owner_uid = None
        record.session = None
        record.signature = None
        record.state = state
        record.message = message

    def _refresh(self, check_sessions=True):
        if self.closed:
            raise BackendError("BackendFailure", "Private remapper is shutting down")
        try:
            discovered = tuple(device for device in self.discovery.scan() if device.seat == SUPPORTED_SEAT)
        except Exception:
            for record in self.running.values():
                if record.handle is not None:
                    self._stop_record(record, "FAILED", "Physical input discovery became unavailable")
            raise BackendError("BackendFailure", "Physical input discovery is unavailable") from None
        devices = {device.fingerprint: device for device in discovered}
        if len(devices) != len(discovered) or len(devices) > 128:
            raise BackendError("BackendFailure", "Physical input inventory is ambiguous or exceeds its bound")
        topology = tuple(sorted((fingerprint, device.signature) for fingerprint, device in devices.items()))
        if self.topology is not None and topology != self.topology:
            self.generation = self.generation + 1 if self.generation < 2**64 - 1 else secrets.randbits(64) or 1
        self.topology = topology
        self.devices = devices
        sessions = {}
        for fingerprint, record in self.running.items():
            self._observe(record)
            if record.handle is None:
                continue
            device = devices.get(fingerprint)
            if device is None or device.signature != record.signature:
                self._stop_record(record, "FAILED", "The applied physical mouse or source topology changed")
            elif check_sessions:
                if record.session not in sessions:
                    sessions[record.session] = self.credentials.session_is_active(record.session)
                if not sessions[record.session]:
                    self._stop_record(record, "STOPPED", "Owning graphical session is no longer locally active")

    def _snapshot(self, fingerprint):
        record = self.running.get(fingerprint)
        known = fingerprint in self.devices
        return {
            "version": 1,
            "state": record.state if record is not None else ("STOPPED" if known else "UNKNOWN"),
            "preset": record.preset if record is not None else None,
            "ownerUid": record.owner_uid if record is not None else None,
            "generation": self.generation,
            "fingerprint": fingerprint if known else None,
            "message": record.message if record is not None else (None if known else "No matching physical mouse is currently discovered"),
        }

    def _expected(self, fingerprint, expected_preset, uid):
        if expected_preset != "":
            preset_identity(expected_preset)
        record = self.running.get(fingerprint)
        actual = record.preset if record is not None else None
        if record is not None and record.owner_uid is not None and record.owner_uid != uid:
            raise BackendError("AccessDenied", "The active private injector belongs to a different user")
        if (expected_preset or None) != actual:
            raise BackendError("Conflict", "The active private preset does not match expectedPreset")
        return record

    def inventory(self):
        with self.lock:
            self._refresh()
            return {"version": 1, "generation": self.generation, "keyboardOutputCodes": sorted(self.engine.keyboard_codes), "devices": [device.public() for device in self.devices.values()]}

    def state(self, fingerprint):
        self._fingerprint(fingerprint)
        with self.lock:
            self._refresh()
            return self._snapshot(fingerprint)

    def apply(self, fingerprint, generation, preset, expected_preset, config_dir, call_info):
        self._fingerprint(fingerprint)
        if not integer(generation, 1, 2**64 - 1):
            raise BackendError("StaleGeneration", "A valid uint64 inventory generation is required")
        with self.lock:
            uid = self.credentials.caller_uid(call_info)
            session = self.credentials.active_session(uid)
            self._refresh()
            if generation != self.generation:
                raise BackendError("StaleGeneration", "Inventory generation changed before Apply")
            device = self.devices.get(fingerprint)
            if device is None:
                raise BackendError("DeviceUnavailable", "The requested physical mouse is not available")
            self._expected(fingerprint, expected_preset, uid)
            data = self.preset_reader.read(config_dir, preset, uid)
            validated = self.validator.validate(data, preset, uid, device)
            try:
                compiled = self.engine.compile(device, validated)
            except BackendError:
                raise
            except Exception:
                raise BackendError("InvalidPreset", "The validated preset could not be compiled by the pinned private engine") from None
            # Recheck after bounded I/O/compilation, before touching the old injector.
            self._refresh()
            if generation != self.generation or self.devices.get(fingerprint) != device:
                raise BackendError("StaleGeneration", "Physical identity changed while preparing Apply")
            if not self.credentials.session_is_active(session):
                raise BackendError("AccessDenied", "Graphical session became inactive while preparing Apply")
            record = self._expected(fingerprint, expected_preset, uid)
            if record is not None and record.preset == validated.stem and record.state == "RUNNING" and record.session == session:
                return self._snapshot(fingerprint)
            if record is None:
                if len(self.running) >= 512:
                    for old_fingerprint, old_record in tuple(self.running.items()):
                        if old_record.handle is None:
                            del self.running[old_fingerprint]
                            break
                    else:
                        raise BackendError("BackendFailure", "Private injector state exceeds its bound")
                record = Running()
                self.running[fingerprint] = record
            self._stop_record(record)
            if not validated.effective_bindings:
                record.message = "Passthrough-only preset; no private injector is active"
                return self._snapshot(fingerprint)
            record.preset = validated.stem
            record.owner_uid = uid
            record.session = session
            record.signature = device.signature
            record.state = "STARTING"
            try:
                # Retain ownership before any process can acquire input descriptors.
                record.handle = self.engine.create_handle(device, compiled, validated)
                record.handle.start()
            except Exception:
                self._stop_record(record, "FAILED", "The private injector could not be started")
                return self._snapshot(fingerprint)
            self._observe(record)
            self._refresh()
            return self._snapshot(fingerprint)

    def stop(self, fingerprint, expected_preset, call_info):
        self._fingerprint(fingerprint)
        with self.lock:
            uid = self.credentials.caller_uid(call_info)
            # Cleanup authorization intentionally does not depend on active logind.
            record = self._expected(fingerprint, expected_preset, uid)
            # A pending cleanup may finish in _refresh; preserve the checked CAS
            # for this Stop rather than turning its confirmed exit into Conflict.
            self._refresh(check_sessions=False)
            if record is not None:
                self._stop_record(record)
            return self._snapshot(fingerprint)

    def maintain(self):
        with self.lock:
            self._refresh()

    def shutdown(self):
        with self.lock:
            errors = []
            for record in self.running.values():
                try:
                    self._stop_record(record)
                except BackendError as error:
                    errors.append(error)
            self.closed = True
            if errors:
                raise errors[0]
