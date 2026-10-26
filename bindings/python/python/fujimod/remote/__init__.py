"""Reference-based camera control over local Rust futures."""
from __future__ import annotations

import asyncio
from dataclasses import asdict, dataclass
import json
from typing import Awaitable, Callable, TypeVar
from time import monotonic_ns

T = TypeVar("T")

from .._native import remote as _native
from .. import recording


@dataclass(frozen=True)
class Menu:
    id: str
    key: int | None = None
    via: Menu | None = None


@dataclass(frozen=True)
class Option:
    item: Menu
    name: str


@dataclass(frozen=True)
class Control:
    id: str


Reference = Menu | Option | Control


def facts_from_selection(selection: dict, manifest: dict | None = None) -> dict:
    return json.loads(_native.facts_from_selection(json.dumps(selection), json.dumps(manifest)))


def menu(id: str | int, *, key: int | None = None, via: Menu | None = None) -> Menu:
    return Menu(str(id), key, via)


def option(item: Menu, name: str) -> Option:
    return Option(item, name)


def control(id: str) -> Control:
    return Control(id)


def _reference(value: Reference | None) -> str:
    if value is None:
        return "null"
    kind = {Menu: "menu", Option: "option", Control: "control"}[type(value)]
    return json.dumps({"kind": kind, "reference": value.id if isinstance(value, Control) else asdict(value)})


@dataclass(frozen=True)
class State:
    mode: int
    menu_active: bool
    node: int
    cursor: int
    current_key: int | None


@dataclass(frozen=True)
class Readiness:
    ready: bool
    pending: list[str]
    reads: dict


@dataclass(frozen=True)
class MenuState:
    node: int
    key: int | None
    index: int | None
    child: int | None
    label: str
    exists: bool
    visible: bool
    selected: bool
    enabled: bool
    value: str | None
    ancestors: list[dict]
    root: str


@dataclass(frozen=True)
class ControlState:
    id: str
    position: str | None
    supported: bool


@dataclass(frozen=True)
class RemoteReadResult:
    state: State
    value: MenuState | ControlState
    reads: dict


@dataclass(frozen=True)
class Capture:
    frame: recording.Frame
    time: int
    clock: str
    output: str
    sequence: int


def _convert(value):
    if isinstance(value, str):
        data = json.loads(value)
        if "state" not in data:
            return Readiness(**data)
        kind = MenuState if data["value"]["kind"] == "menu" else ControlState
        return RemoteReadResult(State(**data["state"]), kind(**data["value"]["value"]), data["reads"])
    if isinstance(value, tuple):
        png, time, clock, output, sequence = value
        return Capture(recording.Frame.from_png(png), time, clock, output, sequence)
    return value


def _input_state(state: str, stage: str | None, position: str | None) -> str:
    if state not in ("pressed", "released", "stage", "position"):
        raise ValueError("Invalid input state")
    value = {"kind": state}
    if state in ("stage", "position"):
        value["value"] = stage if state == "stage" else position
    return json.dumps(value)


def _touch(x: int, y: int, kind: str) -> str:
    return json.dumps({"kind": kind, "value": {"x": x, "y": y}})


_TOUCH_END = json.dumps({"kind": "touch_end"})


async def _drive(task):
    loop = asyncio.get_running_loop()
    try:
        while True:
            wake = loop.create_future()

            def resumed(future=wake):
                def notify():
                    if not future.done():
                        future.set_result(None)
                if not loop.is_closed():
                    loop.call_soon_threadsafe(notify)

            if task.poll(resumed):
                return _convert(task.result())
            await wake
            await asyncio.sleep(0)
    finally:
        task.cancel()


class Camera:
    def __init__(self, native):
        self._native = native

    @classmethod
    def from_board(cls, board, facts: dict, *, quantum: int, window_instructions: int,
                   scanout_node: str, frame_counter_kind: str, frame_counter_pointer: str,
                   output: str = "lcd", observer: Callable[[], dict] | None = None) -> Camera:
        """``observer`` runs before every wait, including the waits inside an
        input's readiness and acceptance. It returns the read set it observed
        (``{"stores": [(lo, hi)], "devices": [name]}``), which joins the wait."""
        options = dict(quantum=quantum, window_instructions=window_instructions,
                       scanout_node=scanout_node, output=output,
                       frame_counter_kind=frame_counter_kind,
                       frame_counter_pointer=frame_counter_pointer)
        observe = None if observer is None else lambda: json.dumps(observer())
        return cls(_native.Camera.from_board(board, json.dumps(facts), json.dumps(options), observe))

    @classmethod
    def from_ff80(cls, session, facts: dict, *, read_bytes: int, write_bytes: int,
                  event_bytes: int, poll_seconds: float = .1, output: str = "lcd",
                  operation_scoped: bool = False) -> Camera:
        def connect():
            return _native.Camera.from_ff80(session, json.dumps(facts), read_bytes, write_bytes,
                                            event_bytes, poll_seconds, output)
        return cls(_ScopedNative(connect) if operation_scoped else connect())

    @property
    def now(self) -> int:
        return self._native.now

    @property
    def blocking(self) -> BlockingCamera:
        return BlockingCamera(self)

    def _task(self, action: str, reference: Reference | None, deadline: int, steps: int = 0):
        return self._native.operation(action, _reference(reference), deadline, steps)

    async def read(self, reference: Reference) -> RemoteReadResult:
        return await _drive(self._task("read", reference, 0))

    async def set_input(self, reference: Control, state: str, *, deadline: int,
                        stage: str | None = None, position: str | None = None) -> None:
        await _drive(self._native.set_input(_reference(reference),
                                            _input_state(state, stage, position), deadline))

    async def wait(self, reads: dict, *, deadline: int) -> None:
        await _drive(self._native.wait_reads(json.dumps(reads), deadline))

    async def readiness(self) -> Readiness:
        return await _drive(self._task("readiness", None, 0))

    async def wait_until_ready(self, *, deadline: int, control: Control | None = None) -> None:
        await _drive(self._task("wait_ready", control, deadline))

    async def wait_for(self, predicate: Callable[[], Awaitable[T]], *, deadline: int,
                       reads: dict, condition: str) -> T:
        """Compose async observations with the remote wait primitive.

        The predicate records its store/device observations in reads on each
        evaluation. Exceptions propagate unchanged, including cancellation.
        """
        while True:
            reads["stores"] = []
            reads["devices"] = []
            value = await predicate()
            if value:
                return value
            if self.now >= deadline:
                raise TimeoutError(f"Timed out waiting for {condition}; read set: {reads}")
            await self.wait(reads, deadline=deadline)

    async def navigate(self, reference: Menu, *, deadline: int) -> RemoteReadResult:
        return await _drive(self._task("navigate", reference, deadline))

    async def select(self, reference: Option, *, deadline: int) -> None:
        await _drive(self._task("select", reference, deadline))

    async def press(self, reference: Control, *, deadline: int) -> None:
        await _drive(self._task("press", reference, deadline))

    async def turn(self, reference: Control, n: int, *, deadline: int) -> None:
        await _drive(self._task("turn", reference, deadline, n))

    async def touch_start(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        await _drive(self._native.set_input(_reference(reference), _touch(x, y, "touch_start"), deadline))

    async def touch_move(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        await _drive(self._native.set_input(_reference(reference), _touch(x, y, "touch_move"), deadline))

    async def touch_end(self, reference: Control, *, deadline: int) -> None:
        await _drive(self._native.set_input(_reference(reference), _TOUCH_END, deadline))

    async def tap(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        await self.drag(reference, [(x, y)], deadline=deadline)

    async def drag(self, reference: Control, points: list[tuple[int, int]], *, deadline: int) -> None:
        await _drive(self._native.touch_gesture(_reference(reference), json.dumps(points), deadline))

    async def frame(self) -> Capture:
        return await _drive(self._task("frame", None, 0))

    async def screenshot(self, *, deadline: int) -> Capture:
        return await _drive(self._task("screenshot", None, deadline))

    async def wait_until(self, predicate: Callable[[RemoteReadResult], bool], reference: Reference,
                         *, deadline: int) -> RemoteReadResult:
        return await _drive(self._native.wait_until(_reference(reference), predicate, deadline))

    async def wait_until_memory(self, predicate: Callable[[list[tuple[int, bytes]]], bool],
                                ranges: list[tuple[int, int]], *, deadline: int,
                                condition: str) -> list[tuple[int, bytes]]:
        """Repeat explicit byte observations and predicate checks in Rust."""
        return await _drive(self._native.wait_until_memory(
            json.dumps(ranges), predicate, deadline, condition))

    async def wait_until_stable_memory(self, ranges: list[tuple[int, int]], *,
                                       stable_spans: list[tuple[int, int]],
                                       equals: list[tuple[int, bytes]], stable_comparisons: int,
                                       deadline: int, condition: str) -> list[tuple[int, bytes]]:
        """Wait in Rust for caller-selected bytes to match and stabilize."""
        encoded_equals = [(address, list(expected)) for address, expected in equals]
        return await _drive(self._native.wait_until_stable_memory_task(
            json.dumps(ranges), json.dumps(stable_spans), json.dumps(encoded_equals),
            stable_comparisons, deadline, condition))

    def close(self) -> None:
        self._native.close()


class BlockingCamera:
    def __init__(self, camera: Camera):
        self._camera = camera

    @property
    def now(self) -> int:
        return self._camera.now

    def _call(self, action: str, reference: Reference | None = None, deadline: int = 0, steps: int = 0):
        return _convert(self._camera._task(action, reference, deadline, steps).blocking())

    def read(self, reference: Reference) -> RemoteReadResult:
        return self._call("read", reference)

    def readiness(self) -> Readiness:
        return self._call("readiness")

    def wait_until_ready(self, *, deadline: int, control: Control | None = None) -> None:
        self._call("wait_ready", control, deadline)

    def navigate(self, reference: Menu, *, deadline: int) -> RemoteReadResult:
        return self._call("navigate", reference, deadline)

    def select(self, reference: Option, *, deadline: int) -> None:
        self._call("select", reference, deadline)

    def set_input(self, reference: Control, state: str, *, deadline: int,
                  stage: str | None = None, position: str | None = None) -> None:
        self._camera._native.set_input(_reference(reference), _input_state(state, stage, position),
                                       deadline).blocking()

    def press(self, reference: Control, *, deadline: int) -> None:
        self._call("press", reference, deadline)

    def turn(self, reference: Control, n: int, *, deadline: int) -> None:
        self._call("turn", reference, deadline, n)

    def touch_start(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self._camera._native.set_input(_reference(reference), _touch(x, y, "touch_start"),
                                       deadline).blocking()

    def touch_move(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self._camera._native.set_input(_reference(reference), _touch(x, y, "touch_move"),
                                       deadline).blocking()

    def touch_end(self, reference: Control, *, deadline: int) -> None:
        self._camera._native.set_input(_reference(reference), _TOUCH_END, deadline).blocking()

    def tap(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self.drag(reference, [(x, y)], deadline=deadline)

    def drag(self, reference: Control, points: list[tuple[int, int]], *, deadline: int) -> None:
        self._camera._native.touch_gesture(_reference(reference), json.dumps(points),
                                           deadline).blocking()

    def frame(self) -> Capture:
        return self._call("frame")

    def screenshot(self, *, deadline: int) -> Capture:
        return self._call("screenshot", deadline=deadline)

    def wait_until(self, predicate: Callable[[RemoteReadResult], bool], reference: Reference,
                   *, deadline: int) -> RemoteReadResult:
        return _convert(self._camera._native.wait_until(_reference(reference), predicate, deadline).blocking())

    def wait_until_memory(self, predicate: Callable[[list[tuple[int, bytes]]], bool],
                          ranges: list[tuple[int, int]], *, deadline: int,
                          condition: str) -> list[tuple[int, bytes]]:
        return self._camera._native.wait_until_memory(
            json.dumps(ranges), predicate, deadline, condition).blocking()

    def wait_until_stable_memory(self, ranges: list[tuple[int, int]], *,
                                 stable_spans: list[tuple[int, int]],
                                 equals: list[tuple[int, bytes]], stable_comparisons: int,
                                 deadline: int, condition: str) -> list[tuple[int, bytes]]:
        encoded_equals = [(address, list(expected)) for address, expected in equals]
        return self._camera._native.wait_until_stable_memory_task(
            json.dumps(ranges), json.dumps(stable_spans), json.dumps(encoded_equals),
            stable_comparisons, deadline, condition).blocking()


class _ScopedTask:
    def __init__(self, task, owner):
        self.task, self.owner = task, owner

    def poll(self, wake):
        return self.task.poll(wake)

    def result(self):
        try:
            return self.task.result()
        finally:
            self.cancel()

    def blocking(self):
        try:
            return self.task.blocking()
        finally:
            self.cancel()

    def cancel(self):
        self.task.cancel()
        if self.owner is not None:
            self.owner.close()
            self.owner = None


class _ScopedNative:
    """Return a borrowed USB session between operations and predicate reads."""
    def __init__(self, connect):
        self.connect = connect
        self.closed = False

    @property
    def now(self):
        return monotonic_ns()

    def close(self):
        self.closed = True

    def _call(self, method, args, deadline):
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            local_deadline = native.now + max(0, deadline - self.now)
            return _ScopedTask(getattr(native, method)(*args, local_deadline), native)
        except BaseException:
            native.close()
            raise

    def operation(self, action, reference, deadline, steps):
        # operation's steps follow its deadline in the native signature.
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            task = native.operation(action, reference, native.now + max(0, deadline - self.now), steps)
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def wait_until(self, reference, predicate, deadline):
        return self._call("wait_until", (reference, predicate), deadline)

    def wait_until_stable_memory_task(self, ranges, stable_spans, equals, stable_comparisons,
                                      deadline, condition):
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            local_deadline = native.now + max(0, deadline - self.now)
            task = native.wait_until_stable_memory_task(
                ranges, stable_spans, equals, stable_comparisons, local_deadline, condition)
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def wait_reads(self, reads, deadline):
        return self._call("wait_reads", (reads,), deadline)

    def set_input(self, reference, state, deadline):
        return self._call("set_input", (reference, state), deadline)

    def touch_gesture(self, reference, points, deadline):
        return self._call("touch_gesture", (reference, points), deadline)
