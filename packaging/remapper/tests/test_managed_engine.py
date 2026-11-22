# SPDX-License-Identifier: GPL-3.0-or-later
"""Actual private adapter with explicit process, source and upstream API fakes.

The Linux suite separately exercises the pinned upstream implementation. These
tests need no engine staging, third-party dependencies or kernel input access.
"""

import asyncio
from contextlib import contextmanager
from dataclasses import replace
import importlib.util
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

PRIVATE = Path(__file__).resolve().parents[1] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))

from backend import Backend
from credentials import Credentials
from strict import EngineStatus, HeldKeys, StrictFailure
from validation import Action, BackendError, Binding, MOUSE_OUTPUT_CODES
from fakes import CALL, CONFIG, KEYS, UID, FakeDiscovery, FakeInput, FakeReader, mouse_fixture, payload_for, session


class RecordingOutput:
    def __init__(self, *args, **kwargs):
        self.events = []
        self.held = HeldKeys()
        self.syns = 0
        self.closed = False

    def can_emit(self, event):
        return event[0] == 1 and event[1] in KEYS | MOUSE_OUTPUT_CODES

    def write(self, *event):
        self.events.append(event)
        self.held.observe(*event)

    def syn(self):
        self.syns += 1

    def close(self):
        self.closed = True


class FakeGlobalUInputs:
    def __init__(self, factory):
        self.devices = {"keyboard": factory(), "mouse": factory()}

    def get_uinput(self, target):
        return self.devices.get(target)

    def write(self, event, target):
        output = self.get_uinput(target)
        output.write(*event)
        output.syn()


class FakeEventReader:
    def __init__(self, context, source, stop_event):
        # Exact constructor contract from pinned EventReader, not _forward_to.
        self.context, self._source, self.stop_event = context, source, stop_event
        self._device_hash = source.origin

    async def handle(self, event):
        if event.type == 1 and event.value == 2:
            return
        self.context.get_forward_uinput(self._device_hash).write(*event.event_tuple)


class FakeKeyHandler:
    def __init__(self, combination, mapping, global_uinputs, **kwargs):
        self.global_uinputs, self.mapping = global_uinputs, mapping
        self._active = False

    def notify(self, event, *args, **kwargs):
        self._active = bool(event.value)
        self.global_uinputs.write((1, self.mapping.output_code, int(self._active)), self.mapping.target_uinput)
        return True

    def reset(self):
        if self._active:
            self.global_uinputs.write((1, self.mapping.output_code, 0), self.mapping.target_uinput)
            self._active = False


class FakeHoldMacro:
    """Pinned Macro/HoldKeys trigger contract, including the reused release latch."""

    def __init__(self, codes):
        self.codes = codes
        self.released = asyncio.Event()
        self.released.set()
        self.running = False
        self.runs = 0
        self.concurrent = 0
        self.max_concurrent = 0

    def press_trigger(self):
        self.released.clear()

    def release_trigger(self):
        self.released.set()

    async def run(self, callback):
        if self.running:
            return
        self.running = True
        self.runs += 1
        self.concurrent += 1
        self.max_concurrent = max(self.max_concurrent, self.concurrent)
        try:
            for code in self.codes:
                callback(1, code, 1)
                await asyncio.sleep(0)
            await self.released.wait()
            for code in reversed(self.codes):
                callback(1, code, 0)
                await asyncio.sleep(0)
        finally:
            self.concurrent -= 1
            self.running = False


class FakeMacroHandler(FakeKeyHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._pressed_keys = {}
        self._macro = FakeHoldMacro(getattr(self.mapping, "macro_codes", (self.mapping.output_code,)))

    def _remember_pressed_keys(self, event):
        type_, code, value = event
        self._pressed_keys[(type_, code)] = value

    async def run_macro(self, handler):
        await self._macro.run(handler)

    def notify(self, event, *args, **kwargs):
        # The actual Linux suite uses these lines from pinned MacroHandler itself.
        if event.is_pressed():
            self._active = True
            self._macro.press_trigger()
            if self._macro.running:
                return True

            def handler(type_, code, value):
                self._remember_pressed_keys((type_, code, value))
                self.global_uinputs.write((type_, code, value), self.mapping.target_uinput)

            asyncio.ensure_future(self.run_macro(handler))
        else:
            self._active = False
            self._macro.release_trigger()
        return True

    def reset(self):
        self._active = False
        for (type_, code), value in self._pressed_keys.items():
            if value == 1:
                self.global_uinputs.write((type_, code, 0), self.mapping.target_uinput)


@contextmanager
def private_adapter():
    contracts = {
        "evdev": {"UInput": RecordingOutput},
        "inputremapper.configs.input_config": {"InputCombination": tuple, "InputConfig": SimpleNamespace},
        "inputremapper.configs.mapping": {"Mapping": SimpleNamespace},
        "inputremapper.configs.preset": {"Preset": object},
        "inputremapper.injection.context": {"Context": object},
        "inputremapper.injection.event_reader": {"EventReader": FakeEventReader},
        "inputremapper.injection.global_uinputs": {"DEFAULT_UINPUTS": {}, "GlobalUInputs": FakeGlobalUInputs, "UInput": RecordingOutput},
        "inputremapper.injection.injector": {"Injector": object, "InjectorCommand": SimpleNamespace(CLOSE=0)},
        "inputremapper.injection.macros.parse": {"Parser": type("Parser", (), {})},
        "inputremapper.injection.macros.argument": {"ArgumentConfig": SimpleNamespace},
        "inputremapper.injection.macros.task": {"Task": object},
        "inputremapper.injection.macros.tasks.hold_keys": {"HoldKeysTask": object},
        "inputremapper.injection.mapping_handlers.key_handler": {"KeyHandler": FakeKeyHandler},
        "inputremapper.injection.mapping_handlers.macro_handler": {"MacroHandler": FakeMacroHandler},
        "inputremapper.injection.mapping_handlers.mapping_handler": {"HandlerEnums": SimpleNamespace(macro=1, key=2)},
        "inputremapper.injection.mapping_handlers.mapping_parser": {"MappingParser": object, "mapping_handler_classes": {}},
        "inputremapper.input_event": {"InputEvent": object},
        "inputremapper.utils": {"get_device_hash": lambda source: source.origin},
    }
    modules = {}
    for name, attributes in contracts.items():
        module = ModuleType(name)
        module.__dict__.update(attributes)
        modules[name] = module
    with patch.dict(sys.modules, modules):
        spec = importlib.util.spec_from_file_location("tested_private_adapter", PRIVATE / "managed_engine.py")
        adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(adapter)
        yield adapter


class FakeConnection:
    def __init__(self):
        self.closed = False

    def close(self):
        self.closed = True

    def poll(self, timeout=0):
        return False


class FakeProcess:
    def __init__(self, state=None, can_exit=True, start_error=False, on_start=None):
        self.pid = None
        self.alive = False
        self.state = state or EngineStatus("STARTING")
        self.can_exit = can_exit
        self.start_error = start_error
        self.on_start = on_start
        self._msg_pipe = [FakeConnection(), FakeConnection()]
        self.commands = []
        self.closed = False

    def start(self):
        self.pid, self.alive = 123, True
        if self.on_start:
            self.on_start()
        if self.start_error:
            raise OSError("Injected startup failure after spawning")

    def is_alive(self):
        return self.alive

    def status(self):
        return self.state

    def stop_injecting(self):
        self.commands.append("close")
        if self.can_exit:
            self.alive = False

    def join(self, timeout):
        self.commands.append(("join", timeout))

    def terminate(self):
        self.commands.append("terminate")

    def kill(self):
        self.commands.append("kill")

    def close(self):
        if self.alive:
            raise AssertionError("A live private process must not be forgotten")
        self.closed = True


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.adapter_context = private_adapter()
        self.adapter = self.adapter_context.__enter__()
        self.addCleanup(self.adapter_context.__exit__, None, None, None)

    def backend(self, process):
        engine = self.adapter.PrivateEngine.__new__(self.adapter.PrivateEngine)
        engine.keyboard_codes, engine.mouse_codes = KEYS, MOUSE_OUTPUT_CODES
        engine.compile = lambda mouse, validated: validated
        self.mouse = mouse_fixture()
        self.reader = FakeReader()
        self.preset = self.reader.put(payload_for(self.mouse))
        credentials = Credentials(lambda sender: UID if sender == CALL["sender"] else 2000, lambda uid: [session(uid)])
        self.backend_instance = Backend(FakeDiscovery([self.mouse]), engine, credentials, self.reader, generation=900)
        self.backend_instance.inventory()
        self.process_patch = patch.object(self.adapter, "ManagedInjector", lambda *args: process)
        self.process_patch.start()
        self.addCleanup(self.process_patch.stop)
        clock = patch.object(self.adapter, "time", SimpleNamespace(monotonic=lambda: next(self.ticks, 10)))
        self.ticks = iter((0, 0, 5))
        clock.start()
        self.addCleanup(clock.stop)
        return self.backend_instance

    def apply(self, expected=""):
        return self.backend_instance.apply(self.mouse.fingerprint, 900, self.preset, expected, CONFIG, CALL)

    def test_passthrough_repeat_uses_context_forwarder_and_syncs(self):
        forward = RecordingOutput()
        source = FakeInput()
        origins = []
        context = SimpleNamespace(get_notify_callbacks=lambda event: [], get_forward_uinput=lambda origin: (origins.append(origin), forward)[1])
        reader = self.adapter.PrivateEventReader(context, source, asyncio.Event())
        event = SimpleNamespace(type=1, code=274, value=2, event_tuple=(1, 274, 2))
        asyncio.run(reader.handle(event))
        self.assertEqual(origins, [source.origin])
        self.assertEqual(forward.events, [(1, 274, 2)])
        self.assertEqual(forward.syns, 1)

    def test_mapped_repeat_stays_consumed_by_upstream_contract(self):
        forward = RecordingOutput()
        context = SimpleNamespace(get_notify_callbacks=lambda event: [object()], get_forward_uinput=lambda origin: forward)
        reader = self.adapter.PrivateEventReader(context, FakeInput(), asyncio.Event())
        asyncio.run(reader.handle(SimpleNamespace(type=1, code=275, value=2)))
        self.assertEqual(forward.events, [])

    def test_startup_timeout_unconfirmed_stop_keeps_handle_owner_and_cas(self):
        process = FakeProcess(can_exit=False)
        backend = self.backend(process)
        with self.assertRaises(BackendError) as error:
            self.apply()
        self.assertEqual(error.exception.code, "BackendFailure")
        record = backend.running[self.mouse.fingerprint]
        self.assertEqual((record.owner_uid, record.preset), (UID, self.preset))
        self.assertIs(record.handle.injector, process)
        self.assertTrue(process.alive)
        self.assertFalse(process.closed)
        for expected in ("", self.preset):
            with self.assertRaises(BackendError):
                self.apply(expected)
            self.assertIs(record.handle.injector, process)
        process.can_exit = True
        stopped = backend.stop(self.mouse.fingerprint, self.preset, CALL)
        self.assertEqual(stopped["state"], "STOPPED")
        self.assertIsNone(record.handle)
        self.assertIsNone(stopped["ownerUid"])
        self.assertTrue(process.closed)

    def test_spawn_failure_after_pid_assignment_retains_handle_until_exit(self):
        process = FakeProcess(can_exit=False, start_error=True)
        backend = self.backend(process)
        with self.assertRaises(BackendError):
            self.apply()
        self.assertIs(backend.running[self.mouse.fingerprint].handle.injector, process)
        process.can_exit = True
        backend.shutdown()
        self.assertFalse(process.alive)

    def test_late_running_after_timed_out_stop_cannot_drop_retained_ownership(self):
        process = FakeProcess(can_exit=False)
        backend = self.backend(process)
        with self.assertRaises(BackendError):
            self.apply()
        record = backend.running[self.mouse.fingerprint]
        process.state = EngineStatus("RUNNING", verified=True)
        self.assertEqual(record.handle.status().state, "FAILED")
        with self.assertRaises(BackendError):
            backend.maintain()
        self.assertIs(record.handle.injector, process)
        self.assertEqual((record.owner_uid, record.preset), (UID, self.preset))
        process.alive = False
        state = backend.state(self.mouse.fingerprint)
        self.assertEqual(state["state"], "FAILED")
        self.assertIsNone(record.handle)
        self.assertIsNone(state["ownerUid"])

    def test_handle_is_owned_before_process_start_and_can_be_observed(self):
        observed = []
        process = FakeProcess(state=EngineStatus("RUNNING", verified=True))
        backend = self.backend(process)
        process.on_start = lambda: observed.append(backend.running[self.mouse.fingerprint].handle)
        state = self.apply()
        self.assertEqual(state["state"], "RUNNING")
        self.assertIs(observed[0], backend.running[self.mouse.fingerprint].handle)
        backend.shutdown()

    def test_startup_timeout_confirmed_exit_clears_ownership(self):
        process = FakeProcess()
        backend = self.backend(process)
        state = self.apply()
        self.assertEqual(state["state"], "FAILED")
        self.assertIsNone(state["ownerUid"])
        self.assertIsNone(state["preset"])
        self.assertIsNone(backend.running[self.mouse.fingerprint].handle)
        self.assertFalse(process.alive)
        self.assertTrue(process.closed)
        self.assertTrue(all(connection.closed for connection in process._msg_pipe))

    def test_never_spawned_handle_can_be_closed_without_leaking_pipes(self):
        process = FakeProcess()
        handle = self.adapter.ManagedHandle(process)
        self.assertEqual(handle.stop().state, "STOPPED")
        self.assertTrue(process.closed)
        self.assertTrue(all(connection.closed for connection in process._msg_pipe))
        self.assertEqual(process.commands, [])

    def test_expected_seat_is_verified_immediately_before_and_after_grab(self):
        mouse = mouse_fixture()
        node = mouse.nodes[0]
        binding = Binding(node.id, node.origin_hash, 275, Action("key", code=30))
        source = FakeInput()
        injector = self.adapter.ManagedInjector.__new__(self.adapter.ManagedInjector)
        injector.group = self.adapter.ManagedGroup(mouse, (binding,))
        injector._devices = [source]
        injector.validated = SimpleNamespace(effective_bindings=(binding,))
        for observations, grabs in (([None], 0), ([node.raw, replace(node.raw, seat="seat1")], 1), ([node.raw, ValueError("uncertain discovery")], 1)):
            with self.subTest(observations=observations):
                source.grabs = source.ungrabs = 0
                sequence = iter(observations)

                def inspect(device):
                    observation = next(sequence)
                    if isinstance(observation, Exception):
                        raise observation
                    return observation

                discovery = SimpleNamespace(inspect=inspect)
                with patch.object(self.adapter, "LinuxDiscovery", lambda *args: discovery):
                    with self.assertRaises(StrictFailure):
                        injector._grab_devices()
                self.assertEqual(source.grabs, grabs)
                self.assertEqual(source.ungrabs, grabs)
                self.assertFalse(source.grabbed)

    def test_managed_group_never_accepts_a_foreign_expected_seat(self):
        mouse = mouse_fixture()
        node = mouse.nodes[0]
        foreign = replace(mouse, nodes=(replace(node, raw=replace(node.raw, seat="seat1")),))
        binding = Binding(node.id, node.origin_hash, 275, Action("key", code=30))
        closed = []
        source = FakeInput()
        source.close = lambda: closed.append(True)
        discovery = SimpleNamespace(inspect=lambda device: foreign.nodes[0].raw)
        with patch.object(self.adapter, "LinuxDiscovery", lambda *args: discovery), patch.object(self.adapter.evdev, "InputDevice", lambda path: source, create=True):
            with self.assertRaises(StrictFailure):
                self.adapter.ManagedGroup(foreign, (binding,)).get_devices()
        self.assertEqual(source.grabs, 0)
        self.assertEqual(closed, [True])

    def test_shared_key_release_waits_for_last_binding_not_repeat_count(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        mapping = SimpleNamespace(output_code=30, target_uinput="keyboard")
        handler_class = getattr(self.adapter, "PrivateKeyHandler", FakeKeyHandler)
        first, second = (handler_class((), mapping, outputs) for _ in range(2))
        for _ in range(20):
            first.notify(SimpleNamespace(value=1))
        second.notify(SimpleNamespace(value=1))
        first.notify(SimpleNamespace(value=0))
        first.reset()
        endpoint = outputs.get_uinput("keyboard")
        self.assertEqual(endpoint.held.codes, [30])
        self.assertEqual(endpoint.events, [(1, 30, 1)])
        second.notify(SimpleNamespace(value=0))
        self.assertEqual(endpoint.held.codes, [])
        self.assertEqual(endpoint.events, [(1, 30, 1), (1, 30, 0)])

    def test_chord_and_plain_key_share_modifier_without_early_release(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        ctrl = SimpleNamespace(output_code=29, target_uinput="keyboard")
        chord = SimpleNamespace(output_code=46, target_uinput="keyboard")
        handler_class = getattr(self.adapter, "PrivateKeyHandler", FakeKeyHandler)
        plain = handler_class((), ctrl, outputs)
        macro = self.adapter.PrivateMacroHandler((), chord, outputs, context=None)
        plain.notify(SimpleNamespace(value=1))
        for code in (29, 42, 46):
            macro.global_uinputs.write((1, code, 1), "keyboard")
        for code in (46, 42, 29):
            macro.global_uinputs.write((1, code, 0), "keyboard")
        endpoint = outputs.get_uinput("keyboard")
        self.assertEqual(endpoint.held.codes, [29])
        self.assertEqual(endpoint.events, [(1, 29, 1), (1, 42, 1), (1, 46, 1), (1, 46, 0), (1, 42, 0)])
        plain.reset()
        self.assertEqual(endpoint.events[-1], (1, 29, 0))

    def test_repeat_does_not_acquire_ownership_and_targets_do_not_collide(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        first, second = self.adapter.BindingUInputs(outputs), self.adapter.BindingUInputs(outputs)
        first.write((1, 30, 2), "keyboard")
        self.assertEqual(outputs.owners, {})
        first.write((1, 30, 1), "keyboard")
        second.write((1, 30, 1), "mouse")
        for _ in range(20):
            first.write((1, 30, 2), "keyboard")
        second.write((1, 30, 0), "mouse")
        first.write((1, 30, 0), "keyboard")
        first.write((1, 30, 0), "keyboard")
        self.assertEqual(outputs.owners, {})
        self.assertEqual(outputs.get_uinput("mouse").events, [(1, 30, 1), (1, 30, 0)])
        keyboard = outputs.get_uinput("keyboard").events
        self.assertEqual(keyboard.count((1, 30, 1)), 1)
        self.assertEqual(keyboard.count((1, 30, 2)), 20)
        self.assertEqual(keyboard.count((1, 30, 0)), 1)

    def test_failed_output_write_keeps_ownership_until_cleanup_can_retry(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        binding = self.adapter.BindingUInputs(outputs)
        endpoint = outputs.get_uinput("keyboard")
        with patch.object(endpoint, "write", side_effect=OSError("fake write failure")):
            with self.assertRaises(StrictFailure):
                binding.write((1, 30, 1), "keyboard")
        self.assertEqual(outputs.owners, {})
        binding.write((1, 30, 1), "keyboard")
        with patch.object(endpoint, "write", side_effect=OSError("fake release failure")):
            with self.assertRaises(StrictFailure):
                binding.write((1, 30, 0), "keyboard")
        self.assertEqual(outputs.owners[("keyboard", 30)], {binding})
        binding.write((1, 30, 0), "keyboard")
        self.assertEqual(outputs.owners, {})

    def test_final_private_teardown_releases_shared_keys_once_in_reverse_order(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        endpoint = self.adapter.TrackedOutput(name="input-remapper keyboard")
        outputs.devices["keyboard"] = endpoint
        first, second = self.adapter.BindingUInputs(outputs), self.adapter.BindingUInputs(outputs)
        first.write((1, 29, 1), "keyboard")
        for code in (29, 42, 46):
            second.write((1, code, 1), "keyboard")
        endpoint.release_and_close()
        self.assertEqual(endpoint.events, [(1, 29, 1), (1, 42, 1), (1, 46, 1), (1, 46, 0), (1, 42, 0), (1, 29, 0)])
        self.assertEqual(endpoint.held.codes, [])
        self.assertTrue(endpoint.closed)

    def chord_handler(self):
        outputs = self.adapter.PrivateUInputs(RecordingOutput)
        mapping = SimpleNamespace(output_code=46, target_uinput="keyboard", macro_codes=(29, 46))
        return self.adapter.PrivateMacroHandler((), mapping, outputs, context=None), outputs

    @staticmethod
    def key_event(value):
        return SimpleNamespace(type=1, value=value, is_pressed=lambda: value != 0)

    @staticmethod
    async def settle_macro():
        for _ in range(48):
            await asyncio.sleep(0)

    def test_chord_release_repress_runs_serially_and_keeps_second_hold(self):
        handler, outputs = self.chord_handler()
        endpoint = outputs.get_uinput("keyboard")

        async def events():
            try:
                handler.notify(self.key_event(1))
                await self.settle_macro()
                handler.notify(self.key_event(0))
                handler.notify(self.key_event(1))
                await self.settle_macro()
                self.assertEqual(endpoint.held.codes, [29, 46])
                self.assertEqual(handler._macro.runs, 2)
                self.assertEqual(handler._macro.max_concurrent, 1)
            finally:
                handler.reset()
                await self.settle_macro()

        asyncio.run(events())
        self.assertEqual(endpoint.held.codes, [])
        self.assertEqual(outputs.owners, {})

    def test_queued_taps_before_start_do_not_borrow_later_press_release(self):
        handler, outputs = self.chord_handler()
        endpoint = outputs.get_uinput("keyboard")

        async def events():
            try:
                for value in (1, 0, 1, 0, 1):
                    handler.notify(self.key_event(value))
                await self.settle_macro()
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1), (1, 46, 0), (1, 29, 0)] * 2 + [(1, 29, 1), (1, 46, 1)])
                self.assertEqual(handler._macro.runs, 3)
                self.assertEqual(handler._macro.max_concurrent, 1)
            finally:
                handler.reset()
                await self.settle_macro()

        asyncio.run(events())
        self.assertEqual(outputs.owners, {})

    def test_repeat_and_duplicate_down_allocate_only_one_macro_activation(self):
        handler, outputs = self.chord_handler()

        async def events():
            before = asyncio.all_tasks()
            handler.notify(self.key_event(1))
            runner = handler._runner
            for _ in range(1024):
                handler.notify(self.key_event(2))
                handler.notify(self.key_event(1))
            self.assertIs(handler._runner, runner)
            self.assertEqual(len(handler._pending), 1)
            self.assertEqual(len(asyncio.all_tasks() - before), 1)
            await self.settle_macro()
            self.assertEqual(handler._macro.runs, 1)
            handler.reset()
            await self.settle_macro()

        asyncio.run(events())
        self.assertEqual(outputs.owners, {})

    def test_macro_activation_backlog_is_bounded_and_overflow_is_fatal(self):
        handler, outputs = self.chord_handler()

        async def events():
            for _ in range(handler.MAX_ACTIVATIONS):
                handler.notify(self.key_event(1))
                handler.notify(self.key_event(0))
            self.assertEqual(len(handler._pending), handler.MAX_ACTIVATIONS)
            with self.assertRaises(StrictFailure) as error:
                handler.notify(self.key_event(1))
            self.assertIn("backlog", str(error.exception))
            self.assertEqual(len(handler._pending), 0)
            with self.assertRaises(StrictFailure):
                handler.notify(self.key_event(1))
            handler.reset()
            await self.settle_macro()
            self.assertIsNone(handler._runner)

        asyncio.run(events())
        self.assertEqual(outputs.owners, {})

    def test_reset_before_runner_starts_can_rearm_without_old_activation(self):
        handler, outputs = self.chord_handler()
        endpoint = outputs.get_uinput("keyboard")

        async def events():
            handler.notify(self.key_event(1))
            handler.reset()
            handler.notify(self.key_event(1))
            await self.settle_macro()
            self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1)])
            self.assertEqual(handler._macro.runs, 1)
            self.assertEqual(handler._macro.max_concurrent, 1)
            handler.reset()
            await self.settle_macro()

        asyncio.run(events())
        self.assertEqual(outputs.owners, {})

    def test_backlog_bound_includes_running_chord_and_overflow_releases_it(self):
        handler, outputs = self.chord_handler()
        endpoint = outputs.get_uinput("keyboard")

        async def events():
            handler.notify(self.key_event(1))
            await self.settle_macro()
            for _ in range(handler.MAX_ACTIVATIONS - 1):
                handler.notify(self.key_event(0))
                handler.notify(self.key_event(1))
            handler.notify(self.key_event(0))
            self.assertEqual(len(handler._pending), handler.MAX_ACTIVATIONS - 1)
            self.assertEqual(endpoint.held.codes, [29, 46])
            with self.assertRaises(StrictFailure):
                handler.notify(self.key_event(1))
            self.assertEqual(endpoint.held.codes, [])
            self.assertEqual(outputs.owners, {})
            await self.settle_macro()
            self.assertIsNone(handler._runner)
            self.assertFalse(handler._macro.running)

        asyncio.run(events())

    def test_async_chord_failure_is_reported_and_releases_its_owned_keys(self):
        handler, outputs = self.chord_handler()
        endpoint = outputs.get_uinput("keyboard")
        write = endpoint.write
        errors = []

        def fail_second_key(*event):
            if event == (1, 46, 1):
                raise OSError("Injected chord output failure")
            write(*event)

        async def events():
            loop = asyncio.get_running_loop()
            loop.set_exception_handler(lambda loop, context: errors.append(context))
            with patch.object(endpoint, "write", fail_second_key):
                handler.notify(self.key_event(1))
                await self.settle_macro()
            self.assertEqual(len(errors), 1)
            self.assertIsInstance(errors[0]["exception"], StrictFailure)
            self.assertEqual(endpoint.held.codes, [])
            self.assertEqual(outputs.owners, {})
            with self.assertRaises(StrictFailure):
                handler.notify(self.key_event(1))

        asyncio.run(events())


if __name__ == "__main__":
    unittest.main()
