"""Always-on phase spans and run costs for walkthrough and emulator execution.

The native ledger owns the tables. Attached boards supply guest progress at
span boundaries and the existing CPU/R-profile counters at guide boundaries.
"""

from __future__ import annotations

import json
import os
import weakref
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from os import PathLike
from typing import Protocol, cast

from ._native import perf as _native

# Weak, so an attached board that is dropped without close() can still be freed.
_boards: list[weakref.ref[object]] = []
_DEFAULT = object()


class _Counters(Protocol):
    def exit_counters(self) -> str: ...


class _Attached(Protocol):
    @property
    def instructions(self) -> int: ...
    @property
    def virtual_ticks(self) -> int: ...
    @property
    def native(self) -> _Counters: ...


def attach_board(board: object) -> None:
    """Make the board the guest-progress source until it closes."""
    detach_board(board)
    _boards.append(weakref.ref(board))
    _snapshot_counters(board)


def detach_board(board: object) -> None:
    for index in range(len(_boards) - 1, -1, -1):
        if (attached := _boards[index]()) is None or attached is board:
            del _boards[index]


def _board() -> object | None:
    while _boards:
        if (board := _boards[-1]()) is not None:
            return board
        _boards.pop()
    return None


def _guest(board: object = _DEFAULT) -> tuple[int, int]:
    board = _board() if board is _DEFAULT else board
    if board is None:
        return 0, 0
    try:
        attached = cast(_Attached, board)
        return attached.instructions, attached.virtual_ticks
    except AttributeError, RuntimeError:
        return 0, 0


def _snapshot_counters(board: object = _DEFAULT) -> None:
    board = _board() if board is _DEFAULT else board
    if board is None:
        return
    try:
        snapshot = cast(_Attached, board).native.exit_counters()
    except RuntimeError:
        return
    _native.set_cpu_counters(snapshot)


def begin_guide(guide_id: str) -> None:
    _native.begin_guide(guide_id)


def snapshot_counters(board: object = _DEFAULT) -> None:
    """Capture the board's existing CPU and R-profile counters."""
    _snapshot_counters(board)


def end_guide(board: object = _DEFAULT) -> dict[str, object]:
    _snapshot_counters(board)
    instructions, ticks = _guest(board)
    _native.end_span(instructions, ticks, [])
    summary = json.loads(_native.end_guide())
    if not isinstance(summary, dict):
        raise RuntimeError("Native guide summary must be an object")
    return cast(dict[str, object], summary)


@contextmanager
def span(
    kind: str,
    wait_kind: str | None = None,
    properties: Mapping[str, int] | None = None,
    board: object = _DEFAULT,
    wait_counter: int | None = None,
) -> Generator[None]:
    """Record one phase span, including guest progress across its body."""
    selected = _board() if board is _DEFAULT else board
    instructions, ticks = _guest(selected)
    _native.begin_span(kind, wait_kind, instructions, ticks, wait_counter)
    try:
        yield
    finally:
        end_instructions, end_ticks = _guest(selected)
        rows = (
            []
            if properties is None
            else [(str(name), int(value)) for name, value in properties.items()]
        )
        _native.end_span(end_instructions, end_ticks, rows)


def add_bytes(counter: str, count: int) -> None:
    if type(count) is not int or count < 0:
        raise ValueError("byte counter increments must be nonnegative integers")
    _native.add_bytes(counter, count)


def begin_wait(description: str) -> int:
    return _native.begin_wait(description)


def record_wait_evaluation(index: int) -> None:
    _native.record_wait_evaluation(index)


def record_wait_stop(index: int, cause: str, device: str | None = None) -> None:
    _native.record_wait_stop(index, cause, device)


def write_chrome_trace(path: str | PathLike[str]) -> None:
    _native.write_chrome_trace(os.fspath(path))
