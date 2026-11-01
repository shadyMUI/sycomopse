# SPDX-License-Identifier: GPL-3.0-or-later
"""Real pinned engine imports/compilation/pipeline; no services or hardware grabs.

Run after fetch-engine.py staging, with the distro's Linux dependencies available.
Only the output/source endpoints below are explicitly injected recording fakes.
Actual evdev/UInput, logind authorization and system-unit tests require a Linux seat.
"""

import asyncio
from dataclasses import replace
from pathlib import Path
import sys
import unittest

PRIVATE = Path(__file__).resolve().parents[2] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bootstrap import load_engine
from strict import StrictFailure
from validation import PresetValidator
from fakes import UID, FakeInput, encoded, mouse_fixture, payload_for, raw_node


class RecordingForward:
    def __init__(self):
        self.events = []

    def write(self, *event):
        self.events.append(event)

    def syn(self):
        pass


class RecordingOutputs:
    def __init__(self):
        self.events = []

    def write(self, event, target):
        self.events.append((target, tuple(event)))


@unittest.skipUnless(sys.platform == "linux", "Real pinned engine integration requires Linux and its distro dependencies")
class LinuxEngineTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.engine = load_engine(require_root=False)
        from inputremapper.utils import get_device_hash
        cls.get_hash = staticmethod(get_device_hash)

    def setUp(self):
        self.source = FakeInput(codes=(272, 273, 274, 275))
        self.origin = self.get_hash(self.source)
        self.mouse = mouse_fixture([replace(raw_node(), origin_hash=self.origin)])
        self.outputs = RecordingOutputs()
        self.forward = RecordingForward()

    def compile(self, output=None):
        payload = payload_for(self.mouse, output)
        stem, data = encoded(payload)
        validated = PresetValidator(self.engine.keyboard_codes).validate(data, stem, UID, self.mouse)
        return self.engine.compile(self.mouse, validated), validated

    def reader(self, preset):
        from inputremapper.injection.context import Context
        from managed_engine import PrivateEventReader, PrivateMappingParser
        context = Context(preset, {self.origin: self.source}, {self.origin: self.forward}, PrivateMappingParser(self.outputs))
        return PrivateEventReader(context, self.source, asyncio.Event()), context

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
        self.assertEqual(set(Parser.TASK_CLASSES), {"hold_keys"})

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

    def test_no_stock_config_daemon_or_reader_is_loaded(self):
        from inputremapper.configs.paths import PathUtils
        for module in ("inputremapper.daemon", "inputremapper.configs.global_config", "inputremapper.gui.reader_service"):
            self.assertNotIn(module, sys.modules)
        with self.assertRaises(RuntimeError):
            PathUtils.get_preset_path("untrusted model name", "untrusted preset")


if __name__ == "__main__":
    unittest.main()
