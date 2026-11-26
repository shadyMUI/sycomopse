# SPDX-License-Identifier: GPL-3.0-or-later

from copy import deepcopy
import hashlib
import json
from pathlib import Path
import stat
import sys
import unittest

PRIVATE = Path(__file__).resolve().parents[1] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))

from strict import HeldKeys, StrictFailure, strict_grab
from validation import Action, BackendError, Binding, MAX_BINDINGS, MAX_PRESET_BYTES, PresetValidator, SecurePresetReader, path_components, preset_identity
from fakes import CONFIG, KEYS, PROFILE, UID, FakeFilesystem, FakeInput, encoded, mouse_fixture, payload_for, raw_node, stem_for
from dataclasses import replace


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.mouse = mouse_fixture()
        self.payload = payload_for(self.mouse)
        self.validator = PresetValidator(KEYS)

    def validate(self, payload=None, data=None, stem=None):
        if data is None:
            stem, data = encoded(payload or self.payload)
        return self.validator.validate(data, stem or stem_for(data), UID, self.mouse)

    def assert_invalid(self, *args, **kwargs):
        with self.assertRaises(BackendError) as error:
            self.validate(*args, **kwargs)
        self.assertEqual(error.exception.code, "InvalidPreset")

    def test_valid_numeric_key_and_media_outputs(self):
        for code in (30, 164):
            with self.subTest(code=code):
                validated = self.validate(payload_for(self.mouse, {"kind": "key", "code": code}))
                self.assertEqual(validated.bindings[0].output, Action("key", code=code))

    def test_targets_are_separate_mouse_and_keyboard_allowlists(self):
        valid = self.validate(payload_for(self.mouse, {"kind": "mouseButton", "code": 272}))
        self.assertEqual(valid.bindings[0].output.code, 272)
        for output in ({"kind": "key", "code": 272}, {"kind": "mouseButton", "code": 30}, {"kind": "mouseButton", "code": 280}, {"kind": "key", "code": 999}):
            with self.subTest(output=output):
                self.assert_invalid(payload_for(self.mouse, output))

    def test_rejects_arbitrary_dsl_scripts_and_extra_output_fields(self):
        for output in ({"kind": "macro", "code": "repeat(9,key(KEY_A))"}, {"kind": "key", "code": "KEY_A"}, {"kind": "key", "code": 30, "script": "anything"}, {"kind": "disabled", "command": "anything"}, {"kind": "default", "path": "/tmp/evil"}, "hold_keys(KEY_A)"):
            with self.subTest(output=output):
                self.assert_invalid(payload_for(self.mouse, output))

    def test_structured_chords_only_accept_unique_numeric_modifiers(self):
        valid = self.validate(payload_for(self.mouse, {"kind": "chord", "modifiers": [29, 42], "key": 46}))
        self.assertEqual(valid.bindings[0].output, Action("chord", modifiers=(29, 42), key=46))
        for modifiers, key in (([], 46), ([29, 29], 46), ([30], 46), (["KEY_LEFTCTRL"], 46), ([True], 46), ([29], 29), ([29], 272), ([29], "KEY_C"), ([29], 999), ([[]], 46)):
            with self.subTest(modifiers=modifiers, key=key):
                self.assert_invalid(payload_for(self.mouse, {"kind": "chord", "modifiers": modifiers, "key": key}))

    def test_autoclicker_validation(self):
        valid = self.validate(payload_for(self.mouse, {"kind": "autoclicker", "cps": 15, "button": 272}))
        self.assertEqual(valid.bindings[0].output, Action("autoclicker", cps=15, button=272))
        for invalid_ac in ({"kind": "autoclicker", "cps": 0, "button": 272}, {"kind": "autoclicker", "cps": 101, "button": 272}, {"kind": "autoclicker", "cps": 15, "button": 30}, {"kind": "autoclicker", "cps": 15}):
            with self.subTest(invalid=invalid_ac):
                self.assert_invalid(payload_for(self.mouse, invalid_ac))

    def test_source_must_belong_to_exact_actual_node_and_origin(self):
        for changed in ({"nodeId": "node-foreign"}, {"originHash": "b" * 32}, {"originHash": None}, {"type": 2}, {"type": True}, {"code": -1}, {"code": 0}, {"code": 30}, {"code": "275"}, {"code": True}, {"code": 2**32}):
            with self.subTest(changed=changed):
                payload = deepcopy(self.payload)
                payload["bindings"][0]["input"].update(changed)
                self.assert_invalid(payload)

    def test_future_schema_extra_fields_and_spoofed_owners_are_rejected(self):
        for changed in ({"schemaVersion": 2}, {"schemaVersion": True}, {"ownerUid": 2000}, {"ownerUid": True}, {"fingerprint": "port:" + "a" * 64}, {"profileId": "not-a-uuid"}, {"autoload": True}):
            with self.subTest(changed=changed):
                payload = deepcopy(self.payload)
                payload.update(changed)
                stem, data = encoded(payload)
                self.assert_invalid(data=data, stem=stem_for(data))

    def test_immutable_hash_is_of_the_actual_json_bytes_not_reserialization(self):
        stem, data = encoded(self.payload)
        pretty = json.dumps(self.payload, indent=2).encode("utf-8")
        self.assert_invalid(data=pretty, stem=stem)
        validated = self.validate(data=pretty, stem=stem_for(pretty))
        self.assertEqual(validated.stem, stem_for(pretty))
        self.assertNotEqual(hashlib.sha256(data).hexdigest(), hashlib.sha256(pretty).hexdigest())

    def test_duplicate_json_fields_nonfinite_values_and_utf8_errors_are_rejected(self):
        stem, data = encoded(self.payload)
        duplicate = data.replace(b'"schemaVersion":1', b'"schemaVersion":1,"schemaVersion":1')
        for altered in (duplicate, data.replace(b'"schemaVersion":1', b'"schemaVersion":NaN'), data + b"\xff", b"[1,2]", b"{}", b"null"):
            with self.subTest(altered=altered[:64]):
                self.assert_invalid(data=altered, stem=stem_for(altered))

    def test_duplicate_sources_are_rejected(self):
        payload = deepcopy(self.payload)
        payload["bindings"].append(deepcopy(payload["bindings"][0]))
        self.assert_invalid(payload)

    def test_primary_click_cannot_be_lost_and_swap_is_allowed(self):
        self.assert_invalid(payload_for(self.mouse, {"kind": "disabled"}, code=272))
        self.assert_invalid(payload_for(self.mouse, {"kind": "autoclicker", "cps": 100, "button": 272}, code=272))
        left = payload_for(self.mouse, {"kind": "mouseButton", "code": 273}, code=272)["bindings"][0]
        right = payload_for(self.mouse, {"kind": "mouseButton", "code": 272}, code=273)["bindings"][0]
        validated = self.validate(payload_for(self.mouse, bindings=[left, right]))
        self.assertEqual(len(validated.effective_bindings), 2)

    def test_autoclick_cps_boundaries_match_editor(self):
        for cps in (1, 80, 100):
            preset = payload_for(self.mouse, {"kind": "autoclicker", "cps": cps, "button": 272})
            self.assertEqual(self.validate(preset).effective_bindings[0].output.cps, cps)
        for cps in (0, 101, 2.5, True):
            self.assert_invalid(payload_for(self.mouse, {"kind": "autoclicker", "cps": cps, "button": 272}))

    def test_auxiliary_left_capability_cannot_replace_primary_passthrough(self):
        mouse = mouse_fixture([raw_node(), replace(raw_node(), node_role="aux", origin_hash="b" * 32, key_codes=(272,), relative_codes=())])
        primary = next(button for button in mouse.buttons if button.code == 272 and button.origin_hash == "a" * 32)
        payload = payload_for(mouse, bindings=[{"input": {"type": 1, "code": 272, "nodeId": primary.node_id, "originHash": primary.origin_hash}, "output": {"kind": "disabled"}}])
        stem, data = encoded(payload)
        with self.assertRaises(BackendError):
            self.validator.validate(data, stem, UID, mouse)

    def test_default_and_omitted_bindings_are_passthrough(self):
        explicit = self.validate(payload_for(self.mouse, {"kind": "default"}, code=272))
        omitted = self.validate(payload_for(self.mouse, bindings=[]))
        self.assertEqual(explicit.effective_bindings, ())
        self.assertEqual(omitted.effective_bindings, ())

    def test_bounded_bytes_and_binding_count(self):
        self.assert_invalid(data=b" " * (MAX_PRESET_BYTES + 1), stem=stem_for(b" " * (MAX_PRESET_BYTES + 1)))
        payload = deepcopy(self.payload)
        payload["bindings"] *= MAX_BINDINGS + 1
        self.assert_invalid(payload)

    def test_preset_stem_owner_uuid_and_extension_are_strict(self):
        stem, _ = encoded(self.payload)
        for invalid in (stem + ".json", "../" + stem, "/" + stem, stem.replace("ph-1000-", "ph-0-"), stem.replace(PROFILE, PROFILE.upper()), stem + "/file", stem.replace("-1000-", "-2000-"), stem + "\0"):
            with self.subTest(stem=invalid):
                with self.assertRaises(BackendError):
                    preset_identity(invalid, UID)


class SecurePathTests(unittest.TestCase):
    def setUp(self):
        self.stem, self.data = encoded(payload_for(mouse_fixture()))
        self.fs = FakeFilesystem(self.stem, self.data)
        self.reader = SecurePresetReader(self.fs)

    def read(self):
        return self.reader.read(CONFIG, self.stem, UID)

    def assert_rejected(self):
        with self.assertRaises(BackendError) as error:
            self.read()
        self.assertEqual(error.exception.code, "InvalidPreset")
        self.assertEqual(self.fs.descriptors, {})

    def test_descriptor_walk_reads_one_inode_without_reopening(self):
        self.assertEqual(self.read(), self.data)
        files = [path for path, _, _ in self.fs.opens if path.endswith(".json")]
        self.assertEqual(files, [self.fs.file_path])
        self.assertEqual(self.fs.descriptors, {})
        self.assertTrue(all(flags & self.fs.O_NOFOLLOW for _, flags, _ in self.fs.opens))

    def test_readonly_immutable_file_is_accepted(self):
        self.fs.nodes[self.fs.file_path]["stat"].st_mode = stat.S_IFREG | 0o400
        self.assertEqual(self.read(), self.data)

    def test_all_symlink_positions_are_rejected_and_fds_closed(self):
        for path in ("/home", CONFIG, CONFIG + "/presets", self.fs.file_path):
            with self.subTest(path=path):
                self.fs = FakeFilesystem(self.stem, self.data)
                self.reader = SecurePresetReader(self.fs)
                self.fs.nodes[path]["symlink"] = True
                self.assert_rejected()

    def test_directory_owner_and_permissions_are_enforced(self):
        for path, owner, mode in ((CONFIG, 0, 0o700), (CONFIG, UID, 0o755), (CONFIG + "/presets", UID, 0o777), ("/home", 2000, 0o755), ("/home", 0, 0o775)):
            with self.subTest(path=path, owner=owner, mode=mode):
                self.fs = FakeFilesystem(self.stem, self.data)
                self.reader = SecurePresetReader(self.fs)
                metadata = self.fs.nodes[path]["stat"]
                metadata.st_uid, metadata.st_mode = owner, stat.S_IFDIR | mode
                self.assert_rejected()

    def test_file_owner_modes_hardlinks_and_special_files_are_rejected(self):
        for owner, mode, links in ((0, stat.S_IFREG | 0o600, 1), (UID, stat.S_IFREG | 0o644, 1), (UID, stat.S_IFREG | 0o700, 1), (UID, stat.S_IFREG | 0o600, 2), (UID, stat.S_IFIFO | 0o600, 1), (UID, stat.S_IFCHR | 0o600, 1)):
            with self.subTest(owner=owner, mode=mode, links=links):
                self.fs = FakeFilesystem(self.stem, self.data)
                self.reader = SecurePresetReader(self.fs)
                metadata = self.fs.nodes[self.fs.file_path]["stat"]
                metadata.st_uid, metadata.st_mode, metadata.st_nlink = owner, mode, links
                self.assert_rejected()
                self.assertEqual(self.fs.read_calls, 0)

    def test_large_file_is_rejected_before_read(self):
        self.fs.nodes[self.fs.file_path]["stat"].st_size = MAX_PRESET_BYTES + 1
        self.assert_rejected()
        self.assertEqual(self.fs.read_calls, 0)

    def test_in_place_mutation_during_descriptor_read_is_rejected(self):
        self.fs.after_read = lambda: setattr(self.fs.nodes[self.fs.file_path]["stat"], "st_mtime_ns", 99)
        self.assert_rejected()

    def test_unchanged_metadata_cannot_bypass_the_byte_hash(self):
        self.fs.nodes[self.fs.file_path]["data"] = self.data.replace(b'"code":30', b'"code":31')
        self.assert_rejected()

    def test_config_path_is_absolute_canonical_and_without_traversal(self):
        for path in ("relative/path", "/", "/home/test/../test", "/home//test", "/home/test/.", "/home/test/", "C:\\Users\\test", "/home/test\0bad", "/home\\test"):
            with self.subTest(path=path):
                with self.assertRaises(BackendError):
                    path_components(path)


class StrictGrabTests(unittest.TestCase):
    def binding(self, node, origin, code=275):
        return Binding(node, origin, code, Action("key", code=30))

    def test_origin_mismatch_is_never_replaced_by_a_guessed_node(self):
        guessed = FakeInput(origin="b" * 32)
        with self.assertRaises(StrictFailure):
            strict_grab({"node-a": guessed}, [self.binding("node-a", "a" * 32)], lambda source: source.origin)
        self.assertEqual(guessed.grabs, 0)

    def test_every_expected_node_and_code_is_required_before_any_grab(self):
        valid = FakeInput()
        for bindings in ([self.binding("node-a", "a" * 32), self.binding("node-missing", "b" * 32)], [self.binding("node-a", "a" * 32, 999)]):
            with self.subTest(bindings=bindings):
                with self.assertRaises(StrictFailure):
                    strict_grab({"node-a": valid}, bindings, lambda source: source.origin)
                self.assertEqual(valid.grabs, 0)

    def test_partial_grab_failure_releases_every_owned_grab(self):
        first = FakeInput()
        busy = FakeInput(origin="b" * 32, fail_grab=True)
        with self.assertRaises(StrictFailure) as error:
            strict_grab({"node-a": first, "node-b": busy}, [self.binding("node-a", first.origin), self.binding("node-b", busy.origin)], lambda source: source.origin)
        self.assertEqual(error.exception.state, "NO_GRAB")
        self.assertFalse(first.grabbed)
        self.assertEqual(first.ungrabs, 1)
        self.assertEqual(busy.ungrabs, 0)

    def test_duplicate_origin_hashes_fail_before_grabs(self):
        first, second = FakeInput(), FakeInput()
        with self.assertRaises(StrictFailure):
            strict_grab({"node-a": first, "node-b": second}, [self.binding("node-a", first.origin), self.binding("node-b", second.origin)], lambda source: source.origin)
        self.assertEqual(first.grabs + second.grabs, 0)

    def test_shared_source_is_grabbed_exactly_once(self):
        first = FakeInput(codes=(274, 275))
        grabbed = strict_grab({"node-a": first}, [self.binding("node-a", first.origin, 274), self.binding("node-a", first.origin, 275)], lambda source: source.origin)
        self.assertEqual(list(grabbed), ["node-a"])
        self.assertEqual(first.grabs, 1)

    def test_held_source_refuses_startup_and_releases_own_grab(self):
        source = FakeInput(pressed=(275,))
        with self.assertRaises(StrictFailure) as error:
            strict_grab({"node-a": source}, [self.binding("node-a", source.origin)], lambda device: device.origin)
        self.assertEqual(error.exception.state, "NO_GRAB")
        self.assertFalse(source.grabbed)
        self.assertEqual(source.ungrabs, 1)

    def test_pregrab_queue_is_not_replayed_as_duplicate_clicks(self):
        source = FakeInput(events=((1, 275, 1), (1, 275, 0)))
        strict_grab({"node-a": source}, [self.binding("node-a", source.origin)], lambda device: device.origin)
        self.assertEqual(source.events, [])

    def test_nonquiescent_queue_fails_and_releases_all_owned_sources(self):
        source = FakeInput(events=[(2, 0, 1)] * 4097)
        with self.assertRaises(StrictFailure) as error:
            strict_grab({"node-a": source}, [self.binding("node-a", source.origin)], lambda device: device.origin)
        self.assertEqual(error.exception.state, "NO_GRAB")
        self.assertFalse(source.grabbed)

    def test_private_held_modifiers_release_in_reverse_order(self):
        held = HeldKeys()
        for code in (29, 42, 46):
            held.observe(1, code, 1)
        held.observe(2, 0, 20)
        events = []
        synced = []
        held.release(lambda *event: events.append(event), lambda: synced.append(True))
        self.assertEqual(events, [(1, 46, 0), (1, 42, 0), (1, 29, 0)])
        self.assertEqual(synced, [True])
        self.assertEqual(held.codes, [])

    def test_already_released_keys_are_not_released_again(self):
        held = HeldKeys()
        held.observe(1, 29, 1)
        held.observe(1, 29, 0)
        events = []
        held.release(lambda *event: events.append(event), lambda: None)
        self.assertEqual(events, [])


if __name__ == "__main__":
    unittest.main()
