# SPDX-License-Identifier: GPL-3.0-or-later
"""Real pinned engine imports/compilation/pipeline; no services or hardware grabs.

Run after fetch-engine.py staging, with the distro's Linux dependencies available.
Only the output/source endpoints below are explicitly injected recording fakes.
Actual evdev/UInput, logind authorization and system-unit tests require a Linux seat.
"""

import asyncio
from contextlib import asynccontextmanager
from dataclasses import replace
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

PRIVATE = Path(__file__).resolve().parents[2] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import bootstrap
from strict import HeldKeys, StrictFailure
from validation import PresetValidator
from fakes import UID, FakeInput, encoded, mouse_fixture, payload_for, raw_node


class RecordingForward:
    def __init__(self):
        self.events = []

    def write(self, *event):
        self.events.append(event)

    def syn(self):
        pass


class RecordingOutput:
    def __init__(self, *, events, name, **kwargs):
        self.name = name
        self.target = name.removeprefix("input-remapper ")
        self.capabilities = events
        self.events = []
        self.log = []
        self.held = HeldKeys()
        self.closed = False

    def can_emit(self, event):
        return event[1] in self.capabilities.get(event[0], ())

    def write(self, *event):
        if self.closed:
            raise AssertionError("No pending chord may write to a closed private output")
        self.events.append(event)
        self.log.append((self.target, event))
        self.held.observe(*event)

    def syn(self):
        pass

    def release_and_close(self):
        self.held.release(self.write, self.syn)
        self.closed = True


@unittest.skipUnless(sys.platform == "linux", "Real pinned engine integration requires Linux and its distro dependencies")
class LinuxEngineTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        staged = os.environ.get("PH_TEST_ENGINE_ROOTFS")
        if staged:
            directory = Path(staged).resolve() / "usr/lib/peripheral-hub/remapper"
            vendor = bootstrap.verify_vendor(directory, require_root=False)
            with patch.object(bootstrap, "verify_vendor", return_value=vendor):
                cls.engine = bootstrap.load_engine(require_root=False)
        else:
            cls.engine = bootstrap.load_engine(require_root=False)
        from inputremapper.utils import get_device_hash
        cls.get_hash = staticmethod(get_device_hash)

    def setUp(self):
        from managed_engine import PrivateUInputs
        self.source = FakeInput(codes=(272, 273, 274, 275, 276))
        self.origin = self.get_hash(self.source)
        self.mouse = mouse_fixture([replace(raw_node(), origin_hash=self.origin, key_codes=self.source.codes)])
        self.outputs = PrivateUInputs(RecordingOutput)
        self.outputs.prepare_single("keyboard")
        self.outputs.prepare_single("mouse")
        self.outputs.events = []
        for endpoint in self.outputs.devices.values():
            endpoint.log = self.outputs.events
        self.forward = RecordingForward()

    def compile(self, output=None, bindings=None):
        payload = payload_for(self.mouse, output, bindings=bindings)
        stem, data = encoded(payload)
        validated = PresetValidator(self.engine.keyboard_codes).validate(data, stem, UID, self.mouse)
        return self.engine.compile(self.mouse, validated), validated

    def reader(self, preset):
        from inputremapper.injection.context import Context
        from managed_engine import PrivateEventReader, PrivateMappingParser
        context = Context(preset, {self.origin: self.source}, {self.origin: self.forward}, PrivateMappingParser(self.outputs))
        return PrivateEventReader(context, self.source, asyncio.Event()), context

    def binding(self, code, output):
        return payload_for(self.mouse, output, code=code)["bindings"][0]

    @staticmethod
    async def settle_chords():
        for _ in range(12):
            await asyncio.sleep(0)

    @asynccontextmanager
    async def buffered_reader(self, preset):
        """Use the pinned read_loop/run, replacing only FD readiness/stat and input."""
        import evdev
        from inputremapper.injection import event_reader
        reader, context = self.reader(preset)
        self.source.fd = 99
        self.source.fileno = lambda: self.source.fd
        loop = asyncio.get_running_loop()
        callbacks = []
        removed = []
        before = asyncio.all_tasks()

        def add_reader(fd, callback):
            self.assertEqual(fd, self.source.fd)
            callbacks.append(callback)

        def feed(*events):
            self.source.events.extend(evdev.InputEvent(0, 0, *event) for event in events)
            callbacks[0]()

        with patch.object(loop, "add_reader", add_reader), patch.object(loop, "remove_reader", removed.append), patch.object(event_reader, "os", SimpleNamespace(stat=lambda fd: SimpleNamespace(st_nlink=1))):
            task = asyncio.create_task(reader.run())
            try:
                await self.settle_chords()
                self.assertEqual(len(callbacks), 1)
                yield feed, task, reader, context
            finally:
                reader.stop_event.set()
                await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 1)
                context.reset()
                pending = asyncio.all_tasks() - before
                for owned in pending:
                    owned.cancel()
                await asyncio.gather(*pending, return_exceptions=True)
                if not removed and not task.cancelled() and task.exception() is not None:
                    # A fatal reader is followed by ManagedInjector closing its loop.
                    loop.remove_reader(self.source.fd)
                self.assertEqual(removed, [self.source.fd])

    async def settle_buffered_chords(self):
        for _ in range(4):
            await self.settle_chords()

    def test_numeric_output_is_real_EV_KEY_mapping_in_memory(self):
        preset, validated = self.compile()
        mapping = next(iter(preset))
        self.assertEqual(mapping.get_output_type_code(), (1, 30))
        self.assertIsNone(mapping.output_symbol)
        self.assertEqual(mapping.input_combination[0].origin_hash, self.origin)
        self.assertIsNone(preset.path)
        self.assertEqual(preset.name, validated.stem)
        with self.assertRaises(RuntimeError):
            preset.load()

    def test_chords_use_only_trusted_hold_keys_and_kernel_names(self):
        from inputremapper.injection.macros.parse import Parser
        preset, _ = self.compile({"kind": "chord", "modifiers": [29, 42], "key": 46})
        symbol = next(iter(preset)).output_symbol
        self.assertEqual(symbol, "hold_keys(" + ",".join(self.engine.key_names[code] for code in (29, 42, 46)) + ")")
        self.assertEqual(set(Parser.TASK_CLASSES), {"hold_keys", "autoclick"})

    def test_actual_mapping_pipeline_preserves_pointer_and_no_duplicate_original(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile()
        reader, context = self.reader(preset)

        async def events():
            await reader.handle(InputEvent.key(275, 1, self.origin))
            await reader.handle(InputEvent.rel(0, 37, self.origin))
            await reader.handle(InputEvent.key(275, 0, self.origin))
            context.reset()

        asyncio.run(events())
        self.assertEqual(self.outputs.events, [("keyboard", (1, 30, 1)), ("keyboard", (1, 30, 0))])
        self.assertEqual(self.forward.events, [(2, 0, 37)])
        self.assertEqual(dict(context.listeners), {})

    def test_passthrough_repeat_is_forwarded_but_dropped_kernel_frames_fail(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile()
        reader, _ = self.reader(preset)

        async def events():
            await reader.handle(InputEvent.from_tuple((1, 274, 2), self.origin))
            with self.assertRaises(StrictFailure):
                await reader.handle(InputEvent.from_tuple((0, 3, 0), self.origin))

        asyncio.run(events())
        self.assertEqual(self.forward.events, [(1, 274, 2)])

    def test_actual_chord_pipeline_releases_modifiers_in_reverse_order(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile({"kind": "chord", "modifiers": [29, 42], "key": 46})
        reader, context = self.reader(preset)

        async def events():
            await reader.handle(InputEvent.key(275, 1, self.origin))
            for _ in range(12):
                await asyncio.sleep(0)
            await reader.handle(InputEvent.key(275, 0, self.origin))
            for _ in range(12):
                await asyncio.sleep(0)
            context.reset()

        asyncio.run(events())
        emitted = [event for target, event in self.outputs.events]
        self.assertEqual(emitted[:3], [(1, 29, 1), (1, 42, 1), (1, 46, 1)])
        self.assertEqual(emitted[3:6], [(1, 46, 0), (1, 42, 0), (1, 29, 0)])
        self.assertEqual(self.forward.events, [])

    def test_actual_shared_key_stays_down_until_last_source_releases(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile(bindings=[self.binding(code, {"kind": "key", "code": 30}) for code in (275, 276)])
        reader, context = self.reader(preset)
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            await reader.handle(InputEvent.key(275, 1, self.origin))
            await reader.handle(InputEvent.key(276, 1, self.origin))
            for _ in range(20):
                await reader.handle(InputEvent.key(275, 1, self.origin))
                await reader.handle(InputEvent.from_tuple((1, 276, 2), self.origin))
            await reader.handle(InputEvent.key(275, 0, self.origin))
            self.assertEqual(endpoint.held.codes, [30])
            self.assertEqual(endpoint.events, [(1, 30, 1)])
            await reader.handle(InputEvent.key(276, 0, self.origin))
            context.reset()

        asyncio.run(events())
        self.assertEqual(endpoint.events, [(1, 30, 1), (1, 30, 0)])
        self.assertEqual(self.outputs.owners, {})
        self.assertEqual(self.forward.events, [])

    def test_actual_chord_does_not_release_modifier_held_by_plain_binding(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile(bindings=[self.binding(275, {"kind": "key", "code": 29}), self.binding(276, {"kind": "chord", "modifiers": [29, 42], "key": 46})])
        reader, context = self.reader(preset)
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            await reader.handle(InputEvent.key(275, 1, self.origin))
            await reader.handle(InputEvent.key(276, 1, self.origin))
            await self.settle_chords()
            self.assertEqual(endpoint.held.codes, [29, 42, 46])
            await reader.handle(InputEvent.key(276, 0, self.origin))
            await self.settle_chords()
            self.assertEqual(endpoint.held.codes, [29])
            self.assertEqual(endpoint.events, [(1, 29, 1), (1, 42, 1), (1, 46, 1), (1, 46, 0), (1, 42, 0)])
            await reader.handle(InputEvent.key(275, 0, self.origin))
            context.reset()

        asyncio.run(events())
        self.assertEqual(endpoint.events[-1], (1, 29, 0))
        self.assertEqual(self.outputs.owners, {})

    def test_actual_two_chords_share_ctrl_in_either_release_order(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile(bindings=[self.binding(275, {"kind": "chord", "modifiers": [29, 42], "key": 46}), self.binding(276, {"kind": "chord", "modifiers": [29], "key": 30})])
        reader, context = self.reader(preset)
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            for released, remaining in ((275, 276), (276, 275)):
                await reader.handle(InputEvent.key(275, 1, self.origin))
                await reader.handle(InputEvent.key(276, 1, self.origin))
                await self.settle_chords()
                self.assertIn(29, endpoint.held.codes)
                await reader.handle(InputEvent.key(released, 0, self.origin))
                await self.settle_chords()
                self.assertIn(29, endpoint.held.codes)
                await reader.handle(InputEvent.key(remaining, 0, self.origin))
                await self.settle_chords()
                self.assertEqual(endpoint.held.codes, [])
            context.reset()

        asyncio.run(events())
        self.assertEqual(endpoint.events.count((1, 29, 1)), 2)
        self.assertEqual(endpoint.events.count((1, 29, 0)), 2)
        self.assertEqual(self.outputs.owners, {})

    def test_actual_context_cleanup_cancels_chord_before_outputs_close(self):
        from inputremapper.input_event import InputEvent
        preset, _ = self.compile(bindings=[self.binding(275, {"kind": "key", "code": 29}), self.binding(276, {"kind": "chord", "modifiers": [29, 42], "key": 46})])
        reader, context = self.reader(preset)
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            await reader.handle(InputEvent.key(275, 1, self.origin))
            await reader.handle(InputEvent.key(276, 1, self.origin))
            await self.settle_chords()
            context.reset()
            pending = asyncio.all_tasks() - {asyncio.current_task()}
            for task in pending:
                task.cancel()
            await asyncio.gather(*pending, return_exceptions=True)
            endpoint.release_and_close()
            await self.settle_chords()

        asyncio.run(events())
        self.assertEqual(endpoint.held.codes, [])
        self.assertEqual(self.outputs.owners, {})
        self.assertEqual(endpoint.events.count((1, 29, 0)), 1)

    def test_actual_buffered_chord_release_repress_keeps_second_press_held(self):
        preset, _ = self.compile({"kind": "chord", "modifiers": [29], "key": 46})
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                feed((1, 275, 1))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [29, 46])
                feed((1, 275, 0), (1, 275, 1))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [29, 46])
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1), (1, 46, 0), (1, 29, 0), (1, 29, 1), (1, 46, 1)])
                self.assertFalse(task.done())
                self.assertFalse(reader.stop_event.is_set())

        asyncio.run(events())
        self.assertEqual(endpoint.held.codes, [])
        self.assertEqual(self.outputs.owners, {})

    def test_actual_buffered_quick_release_repress_release_emits_second_tap(self):
        preset, _ = self.compile({"kind": "chord", "modifiers": [29], "key": 46})
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                feed((1, 275, 1))
                await self.settle_buffered_chords()
                feed((1, 275, 0), (1, 275, 1), (1, 275, 0))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1), (1, 46, 0), (1, 29, 0)] * 2)
                self.assertEqual(endpoint.held.codes, [])
                self.assertEqual(self.outputs.owners, {})
                self.assertFalse(task.done())

        asyncio.run(events())

    def test_actual_buffered_taps_before_worker_start_have_distinct_releases(self):
        preset, _ = self.compile({"kind": "chord", "modifiers": [29], "key": 46})
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                feed((1, 275, 1), (1, 275, 0), (1, 275, 1), (1, 275, 0), (1, 275, 1))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1), (1, 46, 0), (1, 29, 0)] * 2 + [(1, 29, 1), (1, 46, 1)])
                self.assertEqual(endpoint.held.codes, [29, 46])
                feed((1, 275, 0))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [])
                self.assertEqual(endpoint.events.count((1, 29, 0)), 3)

        asyncio.run(events())

    def test_actual_buffered_repress_preserves_shared_ctrl_wheel_and_repeat(self):
        preset, _ = self.compile(bindings=[self.binding(275, {"kind": "chord", "modifiers": [29], "key": 46}), self.binding(276, {"kind": "key", "code": 29})])
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                feed((1, 276, 1), (1, 275, 1))
                await self.settle_buffered_chords()
                feed(*([(1, 275, 2), (1, 275, 1)] * 256), (2, 8, 1), (2, 8, -1), (2, 0, 37))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1)])
                feed((1, 275, 0), (1, 275, 1), (1, 275, 0))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [29])
                self.assertEqual(endpoint.events, [(1, 29, 1), (1, 46, 1), (1, 46, 0), (1, 46, 1), (1, 46, 0)])
                self.assertEqual(self.forward.events, [(2, 8, 1), (2, 8, -1), (2, 0, 37)])
                feed((1, 276, 0))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.events[-1], (1, 29, 0))
                self.assertEqual(self.outputs.owners, {})

        asyncio.run(events())

    def test_actual_buffered_reset_cancels_queued_repress_before_outputs_close(self):
        preset, _ = self.compile({"kind": "chord", "modifiers": [29], "key": 46})
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                feed((1, 275, 1))
                await self.settle_buffered_chords()
                feed((1, 275, 0), (1, 275, 1))
                for _ in range(32):
                    if not self.source.events:
                        break
                    await asyncio.sleep(0)
                self.assertEqual(self.source.events, [])
                context.reset()
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [])
                self.assertEqual(self.outputs.owners, {})
                self.assertEqual(endpoint.events.count((1, 29, 1)), 1)
                self.assertEqual(endpoint.events.count((1, 46, 1)), 1)
                endpoint.release_and_close()
                await self.settle_buffered_chords()

        asyncio.run(events())

    def test_actual_buffered_activation_overflow_fails_closed_and_cleans_outputs(self):
        from managed_engine import PrivateMacroHandler
        preset, _ = self.compile({"kind": "chord", "modifiers": [29], "key": 46})
        endpoint = self.outputs.get_uinput("keyboard")

        async def events():
            async with self.buffered_reader(preset) as (feed, task, reader, context):
                limit = PrivateMacroHandler.MAX_ACTIVATIONS
                feed((1, 275, 1))
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [29, 46])
                feed(*([(1, 275, 0), (1, 275, 1)] * (limit + 1)))
                with self.assertRaises(StrictFailure) as error:
                    await asyncio.wait_for(task, 1)
                self.assertIn("backlog", str(error.exception))
                self.assertEqual(endpoint.held.codes, [])
                self.assertEqual(self.outputs.owners, {})
                context.reset()
                await self.settle_buffered_chords()
                self.assertEqual(endpoint.held.codes, [])
                self.assertEqual(self.outputs.owners, {})
                endpoint.release_and_close()

        asyncio.run(events())

    def test_no_stock_config_daemon_or_reader_is_loaded(self):
        from inputremapper.configs.paths import PathUtils
        for module in ("inputremapper.daemon", "inputremapper.configs.global_config", "inputremapper.gui.reader_service"):
            self.assertNotIn(module, sys.modules)
        with self.assertRaises(RuntimeError):
            PathUtils.get_preset_path("untrusted model name", "untrusted preset")


if __name__ == "__main__":
    unittest.main()
