"""Reference-based camera control over local Rust futures."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Awaitable, Callable
from dataclasses import asdict, dataclass
from time import monotonic_ns
from typing import Protocol, cast

from .. import recording
from .._native import remote as _native


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


def facts_from_selection(
    selection: dict[str, object], manifest: dict[str, object] | None = None
) -> dict[str, object]:
    data = json.loads(
        _native.facts_from_selection(json.dumps(selection), json.dumps(manifest))
    )
    if not isinstance(data, dict):
        raise TypeError("Remote facts must be a JSON object")
    return cast(dict[str, object], data)


def menu(id: str | int, *, key: int | None = None, via: Menu | None = None) -> Menu:
    return Menu(str(id), key, via)


def option(item: Menu, name: str) -> Option:
    return Option(item, name)


def control(id: str) -> Control:
    return Control(id)


def _reference(value: Reference | None) -> str:
    if value is None:
        return "null"
    if isinstance(value, Control):
        kind = "control"
    elif isinstance(value, Option):
        kind = "option"
    else:
        kind = "menu"
    return json.dumps({
        "kind": kind,
        "reference": value.id if isinstance(value, Control) else asdict(value),
    })


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
    reads: dict[str, object]


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
    ancestors: list[dict[str, object]]
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
    reads: dict[str, object]


@dataclass(frozen=True)
class Capture:
    frame: recording.Frame
    time: int
    clock: str
    output: str
    sequence: int


def _json_object(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TypeError("Native remote result must be a JSON object")
    return cast(dict[str, object], value)


def _optional_int(value: object) -> int | None:
    return None if value is None else int(cast(int, value))


def _optional_str(value: object) -> str | None:
    return None if value is None else str(value)


def _convert(value: object) -> object:
    if isinstance(value, str):
        data = _json_object(json.loads(value))
        if "state" not in data:
            return Readiness(
                ready=bool(data["ready"]),
                pending=[str(item) for item in cast(list[object], data["pending"])],
                reads=_json_object(data["reads"]),
            )
        marked = _json_object(data["value"])
        state = _json_object(data["state"])
        if marked["kind"] == "menu":
            value = _json_object(marked["value"])
            return RemoteReadResult(
                state=State(
                    mode=int(cast(int, state["mode"])),
                    menu_active=bool(state["menu_active"]),
                    node=int(cast(int, state["node"])),
                    cursor=int(cast(int, state["cursor"])),
                    current_key=_optional_int(state["current_key"]),
                ),
                value=MenuState(
                    node=int(cast(int, value["node"])),
                    key=_optional_int(value["key"]),
                    index=_optional_int(value["index"]),
                    child=_optional_int(value["child"]),
                    label=str(value["label"]),
                    exists=bool(value["exists"]),
                    visible=bool(value["visible"]),
                    selected=bool(value["selected"]),
                    enabled=bool(value["enabled"]),
                    value=_optional_str(value["value"]),
                    ancestors=[
                        _json_object(entry)
                        for entry in cast(list[object], value["ancestors"])
                    ],
                    root=str(value["root"]),
                ),
                reads=_json_object(data["reads"]),
            )
        value = _json_object(marked["value"])
        return RemoteReadResult(
            state=State(
                mode=int(cast(int, state["mode"])),
                menu_active=bool(state["menu_active"]),
                node=int(cast(int, state["node"])),
                cursor=int(cast(int, state["cursor"])),
                current_key=_optional_int(state["current_key"]),
            ),
            value=ControlState(
                id=str(value["id"]),
                position=_optional_str(value["position"]),
                supported=bool(value["supported"]),
            ),
            reads=_json_object(data["reads"]),
        )
    if isinstance(value, tuple):
        png, time, clock, output, sequence = cast(
            tuple[bytes, int, str, str, int], value
        )
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


class _Task(Protocol):
    def poll(self, wake: Callable[[], None]) -> bool: ...
    def result(self) -> object: ...
    def blocking(self) -> object: ...
    def cancel(self) -> None: ...


class _Camera(Protocol):
    @property
    def now(self) -> int: ...
    def close(self) -> None: ...
    def operation(
        self, action: str, reference: str, deadline: int, steps: int
    ) -> _Task: ...
    def set_input(self, reference: str, state: str, deadline: int) -> _Task: ...
    def touch_gesture(self, reference: str, points: str, deadline: int) -> _Task: ...
    def wait_reads(self, reads: str, deadline: int) -> _Task: ...
    def wait_until(
        self,
        reference: str,
        predicate: Callable[[RemoteReadResult], bool],
        deadline: int,
    ) -> _Task: ...
    def wait_until_memory(
        self,
        ranges: str,
        predicate: Callable[[list[tuple[int, bytes]]], bool],
        deadline: int,
        condition: str,
    ) -> _Task: ...
    def wait_until_stable_memory_task(
        self,
        ranges: str,
        stable_spans: str,
        equals: str,
        stable_comparisons: int,
        deadline: int,
        condition: str,
    ) -> _Task: ...


async def _drive(task: _Task) -> object:
    loop = asyncio.get_running_loop()
    try:
        while True:
            wake = cast(asyncio.Future[None], loop.create_future())

            def resumed(future: asyncio.Future[None] = wake) -> None:
                def notify() -> None:
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
    def __init__(self, native: _Camera) -> None:
        self._native = native

    @classmethod
    def from_board(
        cls,
        board: object,
        facts: dict[str, object],
        *,
        quantum: int,
        window_instructions: int,
        scanout_node: str,
        frame_counter_kind: str,
        frame_counter_pointer: str,
        output: str = "lcd",
        observer: Callable[[], dict[str, object]] | None = None,
    ) -> Camera:
        """``observer`` runs before every wait, including the waits inside an
        input's readiness and acceptance. It returns the read set it observed
        (``{"stores": [(lo, hi)], "devices": [name]}``), which joins the wait."""
        options = dict(
            quantum=quantum,
            window_instructions=window_instructions,
            scanout_node=scanout_node,
            output=output,
            frame_counter_kind=frame_counter_kind,
            frame_counter_pointer=frame_counter_pointer,
        )
        selected = observer

        def encode() -> str:
            assert selected is not None
            return json.dumps(selected())

        observe: Callable[[], str] | None = None if selected is None else encode
        return cls(
            _native.Camera.from_board(
                board, json.dumps(facts), json.dumps(options), observe
            )
        )

    @classmethod
    def from_ff80(
        cls,
        session: object,
        facts: dict[str, object],
        *,
        read_bytes: int,
        write_bytes: int,
        event_bytes: int,
        poll_seconds: float = 0.1,
        output: str = "lcd",
        operation_scoped: bool = False,
    ) -> Camera:
        def connect() -> _native.Camera:
            return _native.Camera.from_ff80(
                session,
                json.dumps(facts),
                read_bytes,
                write_bytes,
                event_bytes,
                poll_seconds,
                output,
            )

        return cls(_ScopedNative(connect) if operation_scoped else connect())

    @property
    def now(self) -> int:
        return self._native.now

    @property
    def blocking(self) -> BlockingCamera:
        return BlockingCamera(self)

    def _task(
        self, action: str, reference: Reference | None, deadline: int, steps: int = 0
    ) -> _Task:
        return self._native.operation(action, _reference(reference), deadline, steps)

    async def read(self, reference: Reference) -> RemoteReadResult:
        return cast(RemoteReadResult, await _drive(self._task("read", reference, 0)))

    async def set_input(
        self,
        reference: Control,
        state: str,
        *,
        deadline: int,
        stage: str | None = None,
        position: str | None = None,
    ) -> None:
        await _drive(
            self._native.set_input(
                _reference(reference), _input_state(state, stage, position), deadline
            )
        )

    async def wait(self, reads: dict[str, object], *, deadline: int) -> None:
        await _drive(self._native.wait_reads(json.dumps(reads), deadline))

    async def readiness(self) -> Readiness:
        return cast(Readiness, await _drive(self._task("readiness", None, 0)))

    async def wait_until_ready(
        self, *, deadline: int, control: Control | None = None
    ) -> None:
        await _drive(self._task("wait_ready", control, deadline))

    async def wait_for[T](
        self,
        predicate: Callable[[], Awaitable[T]],
        *,
        deadline: int,
        reads: dict[str, object],
        condition: str,
    ) -> T:
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
                raise TimeoutError(
                    f"Timed out waiting for {condition}; read set: {reads}"
                )
            await self.wait(reads, deadline=deadline)

    async def navigate(self, reference: Menu, *, deadline: int) -> RemoteReadResult:
        return cast(
            RemoteReadResult, await _drive(self._task("navigate", reference, deadline))
        )

    async def select(self, reference: Option, *, deadline: int) -> None:
        await _drive(self._task("select", reference, deadline))

    async def press(self, reference: Control, *, deadline: int) -> None:
        await _drive(self._task("press", reference, deadline))

    async def turn(self, reference: Control, n: int, *, deadline: int) -> None:
        await _drive(self._task("turn", reference, deadline, n))

    async def touch_start(
        self, reference: Control, x: int, y: int, *, deadline: int
    ) -> None:
        await _drive(
            self._native.set_input(
                _reference(reference), _touch(x, y, "touch_start"), deadline
            )
        )

    async def touch_move(
        self, reference: Control, x: int, y: int, *, deadline: int
    ) -> None:
        await _drive(
            self._native.set_input(
                _reference(reference), _touch(x, y, "touch_move"), deadline
            )
        )

    async def touch_end(self, reference: Control, *, deadline: int) -> None:
        await _drive(
            self._native.set_input(_reference(reference), _TOUCH_END, deadline)
        )

    async def tap(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        await self.drag(reference, [(x, y)], deadline=deadline)

    async def drag(
        self, reference: Control, points: list[tuple[int, int]], *, deadline: int
    ) -> None:
        await _drive(
            self._native.touch_gesture(
                _reference(reference), json.dumps(points), deadline
            )
        )

    async def frame(self) -> Capture:
        return cast(Capture, await _drive(self._task("frame", None, 0)))

    async def screenshot(self, *, deadline: int) -> Capture:
        return cast(Capture, await _drive(self._task("screenshot", None, deadline)))

    async def wait_until(
        self,
        predicate: Callable[[RemoteReadResult], bool],
        reference: Reference,
        *,
        deadline: int,
    ) -> RemoteReadResult:
        return cast(
            RemoteReadResult,
            await _drive(
                self._native.wait_until(_reference(reference), predicate, deadline)
            ),
        )

    async def wait_until_memory(
        self,
        predicate: Callable[[list[tuple[int, bytes]]], bool],
        ranges: list[tuple[int, int]],
        *,
        deadline: int,
        condition: str,
    ) -> list[tuple[int, bytes]]:
        """Repeat explicit byte observations and predicate checks in Rust."""
        return cast(
            list[tuple[int, bytes]],
            await _drive(
                self._native.wait_until_memory(
                    json.dumps(ranges), predicate, deadline, condition
                )
            ),
        )

    async def wait_until_stable_memory(
        self,
        ranges: list[tuple[int, int]],
        *,
        stable_spans: list[tuple[int, int]],
        equals: list[tuple[int, bytes]],
        stable_comparisons: int,
        deadline: int,
        condition: str,
    ) -> list[tuple[int, bytes]]:
        """Wait in Rust for caller-selected bytes to match and stabilize."""
        encoded_equals = [(address, list(expected)) for address, expected in equals]
        return cast(
            list[tuple[int, bytes]],
            await _drive(
                self._native.wait_until_stable_memory_task(
                    json.dumps(ranges),
                    json.dumps(stable_spans),
                    json.dumps(encoded_equals),
                    stable_comparisons,
                    deadline,
                    condition,
                )
            ),
        )

    def close(self) -> None:
        self._native.close()


class BlockingCamera:
    def __init__(self, camera: Camera) -> None:
        self._camera = camera

    @property
    def now(self) -> int:
        return self._camera.now

    def _call(
        self,
        action: str,
        reference: Reference | None = None,
        deadline: int = 0,
        steps: int = 0,
    ) -> object:
        return _convert(
            self._camera._task(action, reference, deadline, steps).blocking()
        )

    def read(self, reference: Reference) -> RemoteReadResult:
        return cast(RemoteReadResult, self._call("read", reference))

    def readiness(self) -> Readiness:
        return cast(Readiness, self._call("readiness"))

    def wait_until_ready(
        self, *, deadline: int, control: Control | None = None
    ) -> None:
        self._call("wait_ready", control, deadline)

    def navigate(self, reference: Menu, *, deadline: int) -> RemoteReadResult:
        return cast(RemoteReadResult, self._call("navigate", reference, deadline))

    def select(self, reference: Option, *, deadline: int) -> None:
        self._call("select", reference, deadline)

    def set_input(
        self,
        reference: Control,
        state: str,
        *,
        deadline: int,
        stage: str | None = None,
        position: str | None = None,
    ) -> None:
        self._camera._native.set_input(
            _reference(reference), _input_state(state, stage, position), deadline
        ).blocking()

    def press(self, reference: Control, *, deadline: int) -> None:
        self._call("press", reference, deadline)

    def turn(self, reference: Control, n: int, *, deadline: int) -> None:
        self._call("turn", reference, deadline, n)

    def touch_start(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self._camera._native.set_input(
            _reference(reference), _touch(x, y, "touch_start"), deadline
        ).blocking()

    def touch_move(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self._camera._native.set_input(
            _reference(reference), _touch(x, y, "touch_move"), deadline
        ).blocking()

    def touch_end(self, reference: Control, *, deadline: int) -> None:
        self._camera._native.set_input(
            _reference(reference), _TOUCH_END, deadline
        ).blocking()

    def tap(self, reference: Control, x: int, y: int, *, deadline: int) -> None:
        self.drag(reference, [(x, y)], deadline=deadline)

    def drag(
        self, reference: Control, points: list[tuple[int, int]], *, deadline: int
    ) -> None:
        self._camera._native.touch_gesture(
            _reference(reference), json.dumps(points), deadline
        ).blocking()

    def frame(self) -> Capture:
        return cast(Capture, self._call("frame"))

    def screenshot(self, *, deadline: int) -> Capture:
        return cast(Capture, self._call("screenshot", deadline=deadline))

    def wait_until(
        self,
        predicate: Callable[[RemoteReadResult], bool],
        reference: Reference,
        *,
        deadline: int,
    ) -> RemoteReadResult:
        return cast(
            RemoteReadResult,
            _convert(
                self._camera._native.wait_until(
                    _reference(reference), predicate, deadline
                ).blocking()
            ),
        )

    def wait_until_memory(
        self,
        predicate: Callable[[list[tuple[int, bytes]]], bool],
        ranges: list[tuple[int, int]],
        *,
        deadline: int,
        condition: str,
    ) -> list[tuple[int, bytes]]:
        return cast(
            list[tuple[int, bytes]],
            self._camera._native.wait_until_memory(
                json.dumps(ranges), predicate, deadline, condition
            ).blocking(),
        )

    def wait_until_stable_memory(
        self,
        ranges: list[tuple[int, int]],
        *,
        stable_spans: list[tuple[int, int]],
        equals: list[tuple[int, bytes]],
        stable_comparisons: int,
        deadline: int,
        condition: str,
    ) -> list[tuple[int, bytes]]:
        encoded_equals = [(address, list(expected)) for address, expected in equals]
        return cast(
            list[tuple[int, bytes]],
            self._camera._native.wait_until_stable_memory_task(
                json.dumps(ranges),
                json.dumps(stable_spans),
                json.dumps(encoded_equals),
                stable_comparisons,
                deadline,
                condition,
            ).blocking(),
        )


class _ScopedTask:
    def __init__(self, task: _Task, owner: _native.Camera | None) -> None:
        self.task, self.owner = task, owner

    def poll(self, wake: Callable[[], None]) -> bool:
        return self.task.poll(wake)

    def result(self) -> object:
        try:
            return self.task.result()
        finally:
            self.cancel()

    def blocking(self) -> object:
        try:
            return self.task.blocking()
        finally:
            self.cancel()

    def cancel(self) -> None:
        self.task.cancel()
        if self.owner is not None:
            self.owner.close()
            self.owner = None


class _ScopedNative:
    """Return a borrowed USB session between operations and predicate reads."""

    def __init__(self, connect: Callable[[], _native.Camera]) -> None:
        self.connect = connect
        self.closed = False

    @property
    def now(self) -> int:
        return monotonic_ns()

    def close(self) -> None:
        self.closed = True

    def _call(
        self, method: str, args: tuple[object, ...], deadline: int
    ) -> _ScopedTask:
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            local_deadline = native.now + max(0, deadline - self.now)
            task = cast(_Task, getattr(native, method)(*args, local_deadline))
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def operation(
        self, action: str, reference: str, deadline: int, steps: int
    ) -> _ScopedTask:
        # operation's steps follow its deadline in the native signature.
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            task = native.operation(
                action, reference, native.now + max(0, deadline - self.now), steps
            )
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def wait_until(
        self,
        reference: str,
        predicate: Callable[[RemoteReadResult], bool],
        deadline: int,
    ) -> _ScopedTask:
        return self._call("wait_until", (reference, predicate), deadline)

    def wait_until_memory(
        self,
        ranges: str,
        predicate: Callable[[list[tuple[int, bytes]]], bool],
        deadline: int,
        condition: str,
    ) -> _ScopedTask:
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            local_deadline = native.now + max(0, deadline - self.now)
            task = native.wait_until_memory(
                ranges, predicate, local_deadline, condition
            )
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def wait_until_stable_memory_task(
        self,
        ranges: str,
        stable_spans: str,
        equals: str,
        stable_comparisons: int,
        deadline: int,
        condition: str,
    ) -> _ScopedTask:
        if self.closed:
            raise RuntimeError("Camera is closed")
        native = self.connect()
        try:
            local_deadline = native.now + max(0, deadline - self.now)
            task = native.wait_until_stable_memory_task(
                ranges,
                stable_spans,
                equals,
                stable_comparisons,
                local_deadline,
                condition,
            )
            return _ScopedTask(task, native)
        except BaseException:
            native.close()
            raise

    def wait_reads(self, reads: str, deadline: int) -> _ScopedTask:
        return self._call("wait_reads", (reads,), deadline)

    def set_input(self, reference: str, state: str, deadline: int) -> _ScopedTask:
        return self._call("set_input", (reference, state), deadline)

    def touch_gesture(self, reference: str, points: str, deadline: int) -> _ScopedTask:
        return self._call("touch_gesture", (reference, points), deadline)
