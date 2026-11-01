# SPDX-License-Identifier: GPL-3.0-or-later

from dataclasses import replace
import inspect
import json
from pathlib import Path
import sys
import threading
import unittest
import xml.etree.ElementTree as ET

PRIVATE = Path(__file__).resolve().parents[1] / "rootfs/usr/lib/peripheral-hub/remapper"
sys.path.insert(0, str(PRIVATE))

from backend import Backend
from bridge import BusCredentials, INTERFACE, MAX_REPLY_BYTES, XML, make_interface, serialize
from credentials import Credentials
from inventory import build_inventory, usb_port_identity
from strict import EngineStatus
from validation import BackendError, MOUSE_OUTPUT_CODES
from fakes import CALL, CONFIG, KEYS, UID, FakeDiscovery, FakeEngine, FakeReader, mouse_fixture, payload_for, raw_node, session


class BackendTests(unittest.TestCase):
    def setUp(self):
        self.mouse = mouse_fixture()
        self.discovery = FakeDiscovery([self.mouse])
        self.engine = FakeEngine()
        self.reader = FakeReader()
        self.users = {":1.10": UID, ":1.20": 2000, ":1.0": 0}
        self.sessions = {UID: [session()], 2000: [session(2000)]}
        self.lookup_calls = []

        def lookup(sender):
            self.lookup_calls.append(sender)
            return self.users[sender]

        self.credentials = Credentials(lookup, lambda uid: self.sessions.get(uid, []))
        self.backend = Backend(self.discovery, self.engine, self.credentials, self.reader, generation=900)
        self.preset = self.reader.put(payload_for(self.mouse))

    def apply(self, preset=None, expected="", call=CALL, generation=None):
        if generation is None:
            generation = self.backend.inventory()["generation"]
        return self.backend.apply(self.mouse.fingerprint, generation, preset or self.preset, expected, CONFIG, call)

    def assert_error(self, code, function, *args, **kwargs):
        with self.assertRaises(BackendError) as error:
            function(*args, **kwargs)
        self.assertEqual(error.exception.code, code)

    def test_exact_inventory_and_real_running_state(self):
        inventory = self.backend.inventory()
        self.assertEqual(set(inventory), {"version", "generation", "keyboardOutputCodes", "devices"})
        self.assertEqual(inventory["keyboardOutputCodes"], sorted(KEYS))
        self.assertNotIn("mouseOutputCodes", inventory)
        state = self.apply()
        self.assertEqual(state["state"], "RUNNING")
        self.assertEqual(state["preset"], self.preset)
        self.assertEqual(state["ownerUid"], UID)
        self.assertEqual(state["fingerprint"], self.mouse.fingerprint)
        self.assertEqual(state["generation"], 900)
        self.assertEqual(set(state), {"version", "state", "preset", "ownerUid", "generation", "fingerprint", "message"})
        self.assertEqual(len(self.reader.reads), 1)

    def test_missing_or_forged_credentials_do_not_reach_files(self):
        for call in ({"uid": UID}, None, {"sender": "io.peripheralhub.Client", "uid": UID}, {"sender": ":1.999"}, {"sender": ":1.0"}):
            with self.subTest(call=call):
                self.assert_error("AccessDenied", self.apply, call=call)
        self.assertEqual(self.reader.reads, [])
        self.assertEqual(self.engine.handles, [])

    def test_uid_field_is_ignored_in_favor_of_bus_credentials(self):
        state = self.apply(call={"sender": ":1.10", "uid": 2000, "ownerUid": 0})
        self.assertEqual(state["ownerUid"], UID)
        self.assertTrue(self.lookup_calls and set(self.lookup_calls) == {":1.10"})

    def test_uid_lookup_must_be_a_real_non_root_uint32(self):
        for uid in (False, True, -1, 0, "1000", 2**32):
            with self.subTest(uid=uid):
                self.users[":1.10"] = uid
                self.assert_error("AccessDenied", self.apply)

    def test_session_restrictions_and_ambiguity_fail_closed(self):
        for changed in ({"active": False}, {"remote": True}, {"type": "tty"}, {"class": "greeter"}, {"state": "closing"}, {"seat": "seat1"}, {"uid": 2000}, {"path": "/fake/session"}):
            with self.subTest(changed=changed):
                self.sessions[UID] = [session(**changed)]
                self.assert_error("AccessDenied", self.apply)
        self.sessions[UID] = [session(), session(path="/org/freedesktop/login1/session/duplicate")]
        self.assert_error("AccessDenied", self.apply)
        self.assertEqual(self.engine.handles, [])

    def test_stale_generation_is_checked_before_preset_io(self):
        self.assert_error("StaleGeneration", self.apply, generation=899)
        self.assertEqual(self.reader.reads, [])

    def test_expected_none_is_not_permission_to_replace_running(self):
        self.apply()
        self.assert_error("Conflict", self.apply)
        self.assertEqual(self.engine.handles[0].stops, 0)
        self.assertEqual(len(self.reader.reads), 1)

    def test_apply_is_idempotent_only_after_valid_cas(self):
        self.apply()
        state = self.apply(expected=self.preset)
        self.assertEqual(state["state"], "RUNNING")
        self.assertEqual(len(self.engine.handles), 1)
        self.assertEqual(self.engine.handles[0].stops, 0)
        self.assertEqual(len(self.reader.reads), 2)

    def test_validated_replacement_stops_only_the_owned_handle(self):
        self.apply()
        other = self.reader.put(payload_for(self.mouse, {"kind": "key", "code": 31}))
        state = self.apply(other, self.preset)
        self.assertEqual(self.engine.handles[0].stops, 1)
        self.assertEqual(self.engine.handles[1].stops, 0)
        self.assertEqual(state["preset"], other)

    def test_invalid_payload_leaves_previous_injector_running(self):
        self.apply()
        payload = payload_for(self.mouse)
        payload["ownerUid"] = 2000
        invalid = self.reader.put(payload)
        self.assert_error("InvalidPreset", self.apply, invalid, self.preset)
        self.assertEqual(self.engine.handles[0].stops, 0)
        self.assertEqual(self.backend.state(self.mouse.fingerprint)["preset"], self.preset)

    def test_compile_failure_does_not_stop_previous_injector(self):
        self.apply()
        self.engine.compile_error = True
        replacement = self.reader.put(payload_for(self.mouse, {"kind": "key", "code": 31}))
        self.assert_error("InvalidPreset", self.apply, replacement, self.preset)
        self.assertEqual(self.engine.handles[0].stops, 0)

    def test_foreign_owner_cannot_apply_or_stop(self):
        self.apply()
        caller = {"sender": ":1.20"}
        self.assert_error("AccessDenied", self.apply, expected=self.preset, call=caller)
        self.assert_error("AccessDenied", self.backend.stop, self.mouse.fingerprint, self.preset, caller)
        self.assertEqual(self.engine.handles[0].stops, 0)

    def test_stop_owner_is_authorized_even_after_logout(self):
        self.apply()
        self.sessions[UID] = []
        state = self.backend.stop(self.mouse.fingerprint, self.preset, CALL)
        self.assertEqual(state["state"], "STOPPED")
        self.assertIsNone(state["preset"])
        self.assertIsNone(state["ownerUid"])
        self.assertEqual(self.engine.handles[0].stops, 1)

    def test_stop_cas_mismatch_never_stops_current_handle(self):
        self.apply()
        self.assert_error("Conflict", self.backend.stop, self.mouse.fingerprint, "", CALL)
        self.assertEqual(self.engine.handles[0].stops, 0)

    def test_maintenance_releases_only_own_session_on_user_switch(self):
        self.apply()
        self.sessions[UID] = [session(active=False)]
        self.backend.maintain()
        state = self.backend.state(self.mouse.fingerprint)
        self.assertEqual(state["state"], "STOPPED")
        self.assertIsNone(state["ownerUid"])
        self.assertEqual(self.engine.handles[0].stops, 1)

    def test_hotplug_changes_generation_and_invalidates_own_applied_sources(self):
        self.apply()
        refreshed = mouse_fixture([replace(raw_node(), path="/dev/input/event8", inode=888, origin_hash="b" * 32)])
        self.assertEqual(refreshed.fingerprint, self.mouse.fingerprint)
        self.discovery.mice = (refreshed,)
        inventory = self.backend.inventory()
        self.assertEqual(inventory["generation"], 901)
        self.assertEqual(self.engine.handles[0].stops, 1)
        self.assert_error("StaleGeneration", self.apply, generation=900)
        self.assertEqual(self.backend.state(self.mouse.fingerprint)["state"], "FAILED")

    def test_hotplug_while_reading_is_rechecked_before_start(self):
        self.backend.inventory()
        self.reader.after_read = lambda: setattr(self.discovery, "mice", (mouse_fixture([replace(raw_node(), inode=999)]),))
        self.assert_error("StaleGeneration", self.apply, generation=900)
        self.assertEqual(self.engine.handles, [])
        self.assertEqual(len(self.reader.reads), 1)

    def test_inactive_session_while_reading_never_starts(self):
        self.reader.after_read = lambda: self.sessions.update({UID: []})
        self.assert_error("AccessDenied", self.apply)
        self.assertEqual(self.engine.handles, [])

    def test_logout_during_startup_is_rechecked_before_running_reply(self):
        self.engine.after_start = lambda: self.sessions.update({UID: []})
        state = self.apply()
        self.assertEqual(state["state"], "STOPPED")
        self.assertIsNone(state["ownerUid"])
        self.assertEqual(self.engine.handles[0].stops, 1)

    def test_hotplug_during_startup_is_rechecked_before_running_reply(self):
        self.engine.after_start = lambda: setattr(self.discovery, "mice", (mouse_fixture([replace(raw_node(), inode=777)]),))
        state = self.apply()
        self.assertEqual(state["state"], "FAILED")
        self.assertIsNone(state["preset"])
        self.assertEqual(self.engine.handles[0].stops, 1)

    def test_unverified_running_is_failed_and_fully_stopped(self):
        self.engine.next_state = EngineStatus("RUNNING", verified=False)
        state = self.apply()
        self.assertEqual(state["state"], "FAILED")
        self.assertIsNone(state["preset"])
        self.assertIsNone(state["ownerUid"])
        self.assertEqual(self.engine.handles[0].stops, 1)

    def test_no_grab_failure_reports_actual_no_owner_or_preset(self):
        self.engine.next_state = EngineStatus("NO_GRAB", "Busy external grab")
        state = self.apply()
        self.assertEqual(state["state"], "NO_GRAB")
        self.assertIsNone(state["preset"])
        self.assertIsNone(state["ownerUid"])

    def test_crashed_child_is_not_reported_running(self):
        self.apply()
        self.engine.handles[0].current = EngineStatus("FAILED", "Child exited")
        state = self.backend.state(self.mouse.fingerprint)
        self.assertEqual(state["state"], "FAILED")
        self.assertIsNone(state["preset"])

    def test_unconfirmed_shutdown_keeps_ownership_and_fails(self):
        self.apply()
        self.engine.handles[0].stop_error = True
        self.assert_error("BackendFailure", self.backend.stop, self.mouse.fingerprint, self.preset, CALL)
        record = self.backend.running[self.mouse.fingerprint]
        self.assertEqual(record.owner_uid, UID)
        self.assertEqual(record.preset, self.preset)
        self.assertIsNotNone(record.handle)

    def test_passthrough_only_apply_never_fabricates_running(self):
        passthrough = self.reader.put(payload_for(self.mouse, bindings=[]))
        state = self.apply(passthrough)
        self.assertEqual(state["state"], "STOPPED")
        self.assertIsNone(state["preset"])
        self.assertEqual(self.engine.handles, [])

    def test_shutdown_stops_only_private_handles(self):
        self.apply()
        self.backend.shutdown()
        self.assertEqual(self.engine.handles[0].stops, 1)
        self.assert_error("BackendFailure", self.backend.inventory)

    def test_restart_epoch_rejects_previous_inventory(self):
        restarted = Backend(self.discovery, self.engine, self.credentials, self.reader, generation=901)
        restarted.inventory()
        self.assert_error("StaleGeneration", restarted.apply, self.mouse.fingerprint, 900, self.preset, "", CONFIG, CALL)

    def test_atomic_concurrent_cas_allows_exactly_one_apply(self):
        self.backend.inventory()
        barrier = threading.Barrier(3)
        outcomes = []

        def worker():
            barrier.wait()
            try:
                outcomes.append(self.apply(generation=900)["state"])
            except BackendError as error:
                outcomes.append(error.code)

        workers = [threading.Thread(target=worker) for _ in range(2)]
        for worker_thread in workers:
            worker_thread.start()
        barrier.wait()
        for worker_thread in workers:
            worker_thread.join(3)
            self.assertFalse(worker_thread.is_alive())
        self.assertCountEqual(outcomes, ["RUNNING", "Conflict"])
        self.assertEqual(len(self.engine.handles), 1)


class ProtocolTests(unittest.TestCase):
    setUp = BackendTests.setUp

    def test_xml_signatures_are_exact_and_no_legacy_methods_exist(self):
        interface = ET.fromstring(XML).find("interface")
        self.assertEqual(interface.attrib["name"], INTERFACE)
        expected = {"GetVersion": ("", "u"), "Inventory": ("", "s"), "State": ("s", "s"), "Apply": ("stsss", "s"), "Stop": ("ss", "s")}
        actual = {}
        for method in interface.findall("method"):
            actual[method.attrib["name"]] = tuple("".join(arg.attrib["type"] for arg in method.findall("arg") if arg.attrib["direction"] == direction) for direction in ("in", "out"))
        self.assertEqual(actual, expected)

    def test_additional_call_info_is_keyword_only_and_required(self):
        def decorator(method):
            method.test_accepts_call_info = True
            return method

        interface = make_interface(self.backend, decorator)
        self.assertEqual(interface.GetVersion(), 1)
        for method in (interface.Apply, interface.Stop):
            self.assertTrue(method.test_accepts_call_info)
            parameter = inspect.signature(method).parameters["call_info"]
            self.assertEqual(parameter.kind, inspect.Parameter.KEYWORD_ONLY)
            self.assertIs(parameter.default, inspect.Parameter.empty)
        with self.assertRaises(TypeError):
            interface.Apply(self.mouse.fingerprint, 900, self.preset, "", CONFIG)
        response = json.loads(interface.Apply(self.mouse.fingerprint, 900, self.preset, "", CONFIG, call_info=CALL))
        self.assertEqual(response["ownerUid"], UID)

    def test_reply_size_is_bounded(self):
        with self.assertRaises(BackendError):
            serialize({"message": "x" * MAX_REPLY_BYTES})

    def test_bus_lookups_call_real_credentials_and_logind_signatures(self):
        class Variant:
            def __init__(self, signature, value):
                self.signature, self.value = signature, value

            def unpack(self):
                return self.value

        class Connection:
            def __init__(self):
                self.calls = []

            def call_sync(self, *arguments):
                self.calls.append(arguments)
                method = arguments[3]
                if method == "GetConnectionUnixUser":
                    return Variant("(u)", (UID,))
                if method == "ListSessions":
                    return Variant("(a(susso))", ([("c1", UID, "test", "seat0", "/org/freedesktop/login1/session/c1")],))
                return Variant("(a{sv})", ({"User": (UID, "/user"), "Seat": ("seat0", "/seat"), "Type": "wayland", "Class": "user", "State": "active", "Remote": False, "Active": True},))

        connection = Connection()
        glib = type("TestGLib", (), {"Variant": Variant})
        gio = type("TestGio", (), {"DBusCallFlags": type("Flags", (), {"NONE": 0})})
        lookups = BusCredentials(connection, glib, gio)
        self.assertEqual(lookups.unix_user(":1.10"), UID)
        self.assertEqual(lookups.sessions(UID)[0]["uid"], UID)
        self.assertEqual([call[3] for call in connection.calls], ["GetConnectionUnixUser", "ListSessions", "GetAll"])
        self.assertTrue(all(call[7] == 1500 for call in connection.calls))
        self.assertEqual(connection.calls[0][4].value, (":1.10",))


class InventoryTests(unittest.TestCase):
    def inventory(self, nodes):
        return build_inventory(nodes, KEYS | MOUSE_OUTPUT_CODES, lambda code: "CODE_" + str(code))

    def test_button_and_node_ids_survive_origin_hash_event_and_name_changes(self):
        original = self.inventory([raw_node()])[0]
        moved = self.inventory([replace(raw_node(), name="Renamed device", origin_hash="b" * 32, path="/dev/input/event99", inode=200)])[0]
        self.assertEqual(original.fingerprint, moved.fingerprint)
        self.assertEqual([button.id for button in original.buttons], [button.id for button in moved.buttons])
        self.assertEqual(original.nodes[0].id, moved.nodes[0].id)
        self.assertNotEqual(original.signature, moved.signature)

    def test_serial_identity_survives_port_change(self):
        original = self.inventory([raw_node()])[0]
        moved = self.inventory([replace(raw_node(), physical_key="usb-port-controller-9", physical_path="/sys/devices/usb9/9-9", phys="usb-controller-9/input0")])[0]
        self.assertEqual(original.identity_strength, "serial")
        self.assertEqual(original.fingerprint, moved.fingerprint)

    def test_port_identity_does_not_use_runtime_usb_bus_numbers(self):
        first = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3.2"
        second = "/sys/devices/pci0000:00/0000:00:14.0/usb9/9-3/9-3.2"
        self.assertEqual(usb_port_identity(first), usb_port_identity(second))
        self.assertNotEqual(usb_port_identity(first), usb_port_identity(first.replace("1-3.2", "1-3.4")))

    def test_missing_and_duplicate_serials_use_distinct_weak_ports(self):
        second = replace(raw_node(), physical_key="usb-port-controller-4", physical_path="/sys/devices/usb1/1-4", path="/dev/input/event6", inode=102)
        inventory = self.inventory([raw_node(), second])
        self.assertEqual(len(inventory), 2)
        self.assertTrue(all(mouse.identity_strength == "port" for mouse in inventory))
        self.assertNotEqual(inventory[0].fingerprint, inventory[1].fingerprint)
        weak = self.inventory([replace(raw_node(), serial=None)])[0]
        self.assertEqual(weak.identity_strength, "port")

    def test_virtual_touchpad_and_keyboard_only_are_excluded(self):
        for node in (replace(raw_node(), virtual=True), replace(raw_node(), touchpad=True), replace(raw_node(), key_codes=(30, 31), relative_codes=())):
            with self.subTest(node=node):
                self.assertEqual(self.inventory([node]), ())

    def test_composite_includes_only_verified_same_physical_identity(self):
        auxiliary = replace(raw_node(), node_role="interface1.1/input1", origin_hash="b" * 32, key_codes=(30, 164), relative_codes=(), path="/dev/input/event6")
        other_keyboard = replace(auxiliary, physical_key="different-device", name="Same model name", origin_hash="c" * 32)
        mouse = self.inventory([raw_node(), auxiliary, other_keyboard])[0]
        self.assertEqual(len(mouse.nodes), 2)
        self.assertEqual({button.code for button in mouse.buttons}, {272, 273, 274, 275, 30, 164})

    def test_duplicate_node_roles_or_origins_are_not_guessed(self):
        original = raw_node()
        for auxiliary in (replace(original, key_codes=(30,), relative_codes=(), origin_hash="b" * 32), replace(original, node_role="other-role", key_codes=(30,), relative_codes=())):
            with self.subTest(auxiliary=auxiliary):
                self.assertEqual(self.inventory([original, auxiliary]), ())

    def test_ambiguous_multi_mouse_receiver_is_excluded(self):
        second = replace(raw_node(), node_role="second-pointer", origin_hash="b" * 32, path="/dev/input/event6")
        self.assertEqual(self.inventory([raw_node(), second]), ())

    def test_conflicting_composite_serials_are_excluded(self):
        auxiliary = replace(raw_node(), node_role="auxiliary", origin_hash="b" * 32, key_codes=(30,), relative_codes=(), serial="other-physical-device")
        self.assertEqual(self.inventory([raw_node(), auxiliary]), ())


if __name__ == "__main__":
    unittest.main()
