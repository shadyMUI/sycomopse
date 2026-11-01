# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Private extension of the verified Input Remapper 2.2.1 Injector.

This deliberately replaces the stock run/grab lifecycle, not the mapping engine.
All output expressions are compiled from validated actions and trusted constants.
"""

import asyncio
import ctypes
import os
import re
import signal
import time

import evdev

from inputremapper.configs.input_config import InputCombination, InputConfig
from inputremapper.configs.mapping import Mapping
from inputremapper.configs.preset import Preset
from inputremapper.injection.context import Context
from inputremapper.injection.event_reader import EventReader
from inputremapper.injection.global_uinputs import DEFAULT_UINPUTS, GlobalUInputs, UInput
from inputremapper.injection.injector import Injector, InjectorCommand
from inputremapper.injection.macros.parse import Parser
from inputremapper.injection.macros.tasks.hold_keys import HoldKeysTask
from inputremapper.injection.mapping_handlers.macro_handler import MacroHandler
from inputremapper.injection.mapping_handlers.mapping_handler import HandlerEnums
from inputremapper.injection.mapping_handlers.mapping_parser import MappingParser, mapping_handler_classes
from inputremapper.input_event import InputEvent
from inputremapper.utils import get_device_hash

from inventory import LinuxDiscovery
from strict import EngineStatus, HeldKeys, STATES, StrictFailure, strict_grab
from validation import BackendError, MOUSE_OUTPUT_CODES


DEV_NAME = "peripheral-hub-remapper"


class PrivatePreset(Preset):
    def __init__(self, stem):
        super().__init__()
        self.stem = stem

    @property
    def name(self):
        return self.stem

    def load(self):
        raise RuntimeError("Managed presets are compiled from validated bytes, never reopened")

    def save(self):
        raise RuntimeError("The root backend never writes user preset files")


class TrackedOutput(UInput):
    def __init__(self, *args, **kwargs):
        kwargs["name"] = DEV_NAME + " " + kwargs["name"].removeprefix("input-remapper ")
        kwargs["phys"] = DEV_NAME
        self.held = HeldKeys()
        super().__init__(*args, **kwargs)

    def write(self, type_, code, value):
        super().write(type_, code, value)
        self.held.observe(type_, code, value)

    def release_and_close(self):
        self.held.release(super().write, self.syn)
        self.close()


class TrackedForward(evdev.UInput):
    def __init__(self, *args, **kwargs):
        self.held = HeldKeys()
        super().__init__(*args, **kwargs)

    def write(self, type_, code, value):
        super().write(type_, code, value)
        self.held.observe(type_, code, value)

    def release_and_close(self):
        self.held.release(super().write, self.syn)
        self.close()


class PrivateUInputs(GlobalUInputs):
    def write(self, event, target_uinput):
        output = self.get_uinput(target_uinput)
        if output is None or not output.can_emit(event):
            raise StrictFailure("Private output cannot emit the validated code")
        try:
            super().write(event, target_uinput)
        except Exception:
            raise StrictFailure("Private output injection failed") from None


class PrivateMacroHandler(MacroHandler):
    async def run_macro(self, handler):
        # Stock MacroHandler logs and swallows failures. Propagate to worker teardown.
        await self._macro.run(handler)

    def reset(self):
        self._macro.release_trigger()
        super().reset()


class PrivateMappingParser(MappingParser):
    def parse_mappings(self, preset, context):
        result = super().parse_mappings(preset, context)
        for mapping in preset:
            for source in mapping.input_combination:
                if source not in result or not result[source]:
                    raise StrictFailure("The pinned engine did not compile every expected mapping")
        return result


class PrivateEventReader(EventReader):
    async def handle(self, event):
        if event.type == 0 and event.code == 3:  # SYN_DROPPED
            raise StrictFailure("Kernel input events were dropped; private outputs require release")
        if event.type == 1 and event.value == 2 and not self.context.get_notify_callbacks(event):
            self._forward_to.write(*event.event_tuple)
            self._forward_to.syn()
            return
        await super().handle(event)

    async def run(self):
        async for event in self.read_loop():
            # Ordered forwarding; no fire-and-forget event logging or stream export.
            await self.handle(InputEvent.from_event(event, origin_hash=self._device_hash))
        if not self.stop_event.is_set():
            raise StrictFailure("An applied input source was disconnected")


class ManagedGroup:
    def __init__(self, mouse, bindings):
        self.key = self.name = mouse.fingerprint
        required = {binding.node_id for binding in bindings}
        self.nodes = tuple(node for node in mouse.nodes if node.id in required)
        self.paths = [node.raw.path for node in self.nodes]

    def get_devices(self):
        discovery = LinuxDiscovery(evdev, get_device_hash)
        devices = []
        try:
            for node in self.nodes:
                source = evdev.InputDevice(node.raw.path)
                devices.append(source)
                if discovery.inspect(source) != node.raw:
                    raise StrictFailure("An expected physical node changed before exclusive grab")
            return devices
        except Exception:
            for device in devices:
                device.close()
            raise StrictFailure("An expected physical node is missing or could not be verified") from None


class ManagedInjector(Injector):
    def __init__(self, mouse, preset, validated, inherited_fds=()):
        self.validated = validated
        self.mouse = mouse
        self.parent_pid = os.getpid()
        self.inherited_fds = inherited_fds
        self.expected_nodes = sorted({binding.node_id for binding in validated.effective_bindings})
        self.last_status = EngineStatus("UNKNOWN")
        outputs = PrivateUInputs(TrackedOutput)
        super().__init__(ManagedGroup(mouse, validated.effective_bindings), preset, PrivateMappingParser(outputs))

    def _find_input_device_fallback(self, input_config):
        raise StrictFailure("Origin hash fallback is forbidden for managed presets")

    def _update_preset(self):
        for mapping in self.preset:
            for source in mapping.input_combination:
                if source.origin_hash is None or self._find_input_device(source) is None:
                    raise StrictFailure("An exact source originHash is required for every managed mapping")

    def _grab_devices(self):
        sources = {node.id: device for node, device in zip(self.group.nodes, self._devices)}
        return strict_grab(sources, self.validated.effective_bindings, get_device_hash)

    def stop_injecting(self):
        # Do not call the stock numlock decorator (xset/numlockx) in a root service.
        try:
            self._msg_pipe[1].send(InjectorCommand.CLOSE)
        except (BrokenPipeError, EOFError, OSError):
            pass

    def status(self):
        try:
            while self._msg_pipe[1].poll():
                message = self._msg_pipe[1].recv()
                if type(message) is not dict or message.get("state") not in STATES:
                    self.last_status = EngineStatus("FAILED", "Invalid private injector state report")
                    continue
                running = message["state"] == "RUNNING"
                verified = (
                    message.get("fingerprint") == self.mouse.fingerprint
                    and message.get("preset") == self.validated.stem
                    and message.get("nodeIds") == self.expected_nodes
                )
                if running and not verified:
                    self.last_status = EngineStatus("FAILED", "Injector did not verify the exact physical sources")
                else:
                    self.last_status = EngineStatus(message["state"], message.get("message"), running and verified)
        except (EOFError, OSError):
            pass
        alive = self.is_alive()
        if self.last_status.state == "UNKNOWN" and alive:
            self.last_status = EngineStatus("STARTING")
        elif self.last_status.state == "UNKNOWN" and self.pid is not None and not alive:
            self.last_status = EngineStatus("FAILED", "Private injector exited before verifying its sources")
        elif self.last_status.active and not alive:
            self.last_status = EngineStatus("FAILED", "Private injector exited without a completed cleanup report")
        return self.last_status

    def _report(self, state, message=None, node_ids=()):
        try:
            self._msg_pipe[0].send({"state": state, "message": message, "fingerprint": self.mouse.fingerprint, "preset": self.validated.stem, "nodeIds": sorted(node_ids)})
        except (BrokenPipeError, EOFError, OSError):
            pass

    def _create_forwarding_device(self, source):
        try:
            return TrackedForward(
                name=source.name[:80], events=self._copy_capabilities(source),
                phys=(DEV_NAME + "/" + (source.phys or ""))[:128],
                vendor=source.info.vendor, product=source.info.product,
                version=source.info.version, bustype=source.info.bustype,
                input_props=source.input_props(),
            )
        except (TypeError, AttributeError):
            raise StrictFailure("python-evdev must support forwarding input_props", "UPGRADE_EVDEV") from None

    async def _serve(self, sources):
        readers = [PrivateEventReader(self.context, source, self._stop_event) for source in sources.values()]
        tasks = [asyncio.create_task(reader.run()) for reader in readers]
        control = asyncio.create_task(self._listen())
        tasks.append(control)
        self._report("RUNNING", node_ids=sources)
        try:
            done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in done:
                task.result()
            if self._fatal_async_error:
                raise StrictFailure("Private chord or event processing failed")
            if not self._stop_event.is_set():
                raise StrictFailure("A private input reader stopped unexpectedly")
        finally:
            self._stop_event.set()
            for task in tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)

    async def _listen(self):
        loop = asyncio.get_running_loop()
        ready = asyncio.Event()
        loop.add_reader(self._msg_pipe[0].fileno(), ready.set)
        try:
            while not self._stop_event.is_set():
                waiter = asyncio.create_task(ready.wait())
                stopped = asyncio.create_task(self._stop_event.wait())
                _, pending = await asyncio.wait((waiter, stopped), return_when=asyncio.FIRST_COMPLETED)
                for task in pending:
                    task.cancel()
                await asyncio.gather(*pending, return_exceptions=True)
                if self._stop_event.is_set():
                    return
                ready.clear()
                try:
                    if self._msg_pipe[0].recv() == InjectorCommand.CLOSE:
                        self._stop_event.set()
                        return
                except (EOFError, OSError):
                    self._stop_event.set()
                    return
        finally:
            loop.remove_reader(self._msg_pipe[0].fileno())

    def run(self):
        self._msg_pipe[1].close()
        for descriptor in self.inherited_fds:
            if descriptor not in (0, 1, 2, self._msg_pipe[0].fileno()):
                try:
                    os.close(descriptor)
                except OSError:
                    pass
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        self._stop_event = asyncio.Event()
        self._fatal_async_error = False
        sources = {}
        forwards = {}
        self._devices = []
        state = "STOPPED"
        message = None

        def asynchronous_failure(*args):
            self._fatal_async_error = True
            self._stop_event.set()

        loop.set_exception_handler(asynchronous_failure)
        for signum in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(signum, self._stop_event.set)
        try:
            # If the singleton parent crashes, its child must release its own grabs.
            libc = ctypes.CDLL(None, use_errno=True)
            if libc.prctl(1, signal.SIGTERM, 0, 0, 0) != 0 or os.getppid() != self.parent_pid:
                raise StrictFailure("Private injector parent could not be verified")
            self._devices = self.group.get_devices()
            self._update_preset()
            sources = self._grab_devices()
            by_hash = {get_device_hash(source): source for source in sources.values()}
            for origin_hash, source in by_hash.items():
                forwards[origin_hash] = self._create_forwarding_device(source)
            for mapping in self.preset:
                self.mapping_parser.global_uinputs.prepare_single(mapping.target_uinput)
            self.context = Context(self.preset, by_hash, forwards, self.mapping_parser)
            if self._stop_event.is_set():
                raise StrictFailure("Injector was stopped before startup completed")
            loop.run_until_complete(self._serve(sources))
        except StrictFailure as error:
            state, message = error.state, str(error)
        except Exception:
            state, message = "FAILED", "Private injector initialization or event processing failed"
        finally:
            self._stop_event.set()
            if self.context is not None:
                try:
                    self.context.reset()
                except Exception:
                    state, message = "FAILED", "Private output reset required device teardown"
            # Cancel chord tasks before a final release of every private output key.
            pending = tuple(asyncio.all_tasks(loop))
            for task in pending:
                task.cancel()
            if pending:
                loop.run_until_complete(asyncio.gather(*pending, return_exceptions=True))
            for output in (*forwards.values(), *self.mapping_parser.global_uinputs.devices.values()):
                try:
                    output.release_and_close()
                except OSError:
                    pass
            for source in sources.values():
                try:
                    source.ungrab()
                except OSError:
                    pass
            for source in self._devices:
                source.close()
            loop.close()
            self._report(state, message)
            self._msg_pipe[0].close()


class ManagedHandle:
    def __init__(self, injector):
        self.injector = injector
        self.closed_status = None

    def status(self):
        return self.closed_status if self.closed_status is not None else self.injector.status()

    def stop(self):
        if self.closed_status is not None:
            return self.closed_status
        injector = self.injector
        if injector.is_alive():
            injector.stop_injecting()
            injector.join(3)
        if injector.is_alive():
            injector.terminate()  # SIGTERM still runs the child's own cleanup handler.
            injector.join(3)
        if injector.is_alive():
            injector.kill()  # Last resort closes only this private child's kernel fds.
            injector.join(2)
        if injector.is_alive():
            raise RuntimeError("Private injector did not terminate")
        injector.status()
        injector._msg_pipe[1].close()
        injector.close()
        self.closed_status = EngineStatus("STOPPED")
        return self.closed_status


class PrivateEngine:
    def __init__(self, evdev_module):
        self.evdev = evdev_module
        named_keys = {code for name, code in evdev_module.ecodes.ecodes.items() if re.fullmatch(r"KEY_[A-Z0-9_]+", name) and name not in ("KEY_MAX", "KEY_CNT", "KEY_RESERVED", "KEY_MIN_INTERESTING") and type(code) is int and 0 < code <= 0x2FF}
        self.keyboard_codes = frozenset(DEFAULT_UINPUTS["keyboard"][1]) & named_keys
        self.mouse_codes = frozenset(DEFAULT_UINPUTS["mouse"][1])
        if self.mouse_codes != MOUSE_OUTPUT_CODES:
            raise RuntimeError("Pinned engine mouse output capabilities changed")
        self.key_names = {}
        for name, code in sorted(self.evdev.ecodes.ecodes.items()):
            if re.fullmatch(r"KEY_[A-Z0-9_]+", name) and name not in ("KEY_MAX", "KEY_CNT", "KEY_RESERVED", "KEY_MIN_INTERESTING") and code in self.keyboard_codes:
                self.key_names.setdefault(code, name)
        if set(self.key_names) != self.keyboard_codes:
            raise RuntimeError("Pinned numeric keyboard outputs have no trusted kernel constants")
        Parser.TASK_CLASSES = {"hold_keys": HoldKeysTask}
        mapping_handler_classes[HandlerEnums.macro] = PrivateMacroHandler

    def compile(self, mouse, validated):
        if validated.fingerprint != mouse.fingerprint:
            raise BackendError("InvalidPreset", "Cannot compile a preset for a different physical mouse")
        preset = PrivatePreset(validated.stem)
        for binding in validated.effective_bindings:
            source = InputConfig(type=1, code=binding.code, origin_hash=binding.origin_hash)
            options = {"input_combination": InputCombination([source]), "target_uinput": "keyboard", "macro_key_sleep_ms": 0}
            output = binding.output
            if output.kind in ("key", "mouseButton"):
                options.update(output_type=1, output_code=output.code, target_uinput="mouse" if output.kind == "mouseButton" else "keyboard")
            elif output.kind == "disabled":
                options["output_symbol"] = "disable"
            elif output.kind == "chord":
                names = [self.key_names[code] for code in (*output.modifiers, output.key)]
                options["output_symbol"] = "hold_keys(" + ",".join(names) + ")"
            else:
                raise BackendError("InvalidPreset", "Only validated, managed actions can be compiled")
            preset.add(Mapping(**options))
        return preset

    def start(self, mouse, compiled, validated):
        injector = ManagedInjector(mouse, compiled, validated, getattr(self, "inherited_fds", ()))
        handle = ManagedHandle(injector)
        try:
            injector.start()
            injector._msg_pipe[0].close()
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline:
                status = handle.status()
                if status.state != "STARTING":
                    return handle
                injector._msg_pipe[1].poll(0.05)
            handle.stop()
            handle.closed_status = EngineStatus("FAILED", "Private injector startup timed out; its devices were released")
            return handle
        except Exception:
            if injector.pid is not None:
                handle.stop()
            else:
                for connection in injector._msg_pipe:
                    connection.close()
            raise
