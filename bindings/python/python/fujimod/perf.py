"""Always-on phase spans and run costs for walkthrough and emulator execution.

The native ledger owns the tables. Attached boards supply guest progress at
span boundaries and the existing CPU/R-profile counters at guide boundaries.
"""

from __future__ import annotations

import json
import os
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from os import PathLike
from typing import Any

from ._native import perf as _native

_boards: list[Any] = []


def attach_board(board: object) -> None:
    """Make the board the guest-progress source until it closes."""
    detach_board(board)
    _boards.append(board)


def detach_board(board: object) -> None:
    for index in range(len(_boards) - 1, -1, -1):
        if _boards[index] is board:
            del _boards[index]


def _board() -> Any | None:
    return _boards[-1] if _boards else None


def _guest() -> tuple[int, int]:
    board = _board()
    if board is None:
        return 0, 0
    try:
        return board.instructions, board.virtual_ticks
    except RuntimeError:
        return 0, 0


def _snapshot_counters() -> None:
    board = _board()
    if board is None:
        return
    try:
        snapshot = board.native.exit_counters()
    except RuntimeError:
        return
    _native.set_cpu_counters(snapshot)


def begin_guide(guide_id: str) -> None:
    _native.begin_guide(guide_id)
    _snapshot_counters()


def end_guide() -> dict[str, object]:
    _snapshot_counters()
    instructions, ticks = _guest()
    _native.end_span(instructions, ticks, [])
    summary = json.loads(_native.end_guide())
    if not isinstance(summary, dict):
        raise RuntimeError("Native guide summary must be an object")
    return summary


@contextmanager
def span(
    kind: str,
    wait_kind: str | None = None,
    properties: Mapping[str, int] | None = None,
) -> Iterator[None]:
    """Record one phase span, including guest progress across its body."""
    instructions, ticks = _guest()
    _native.begin_span(kind, wait_kind, instructions, ticks)
    try:
        yield
    finally:
        end_instructions, end_ticks = _guest()
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


def write_chrome_trace(path: str | PathLike[str]) -> None:
    _native.write_chrome_trace(os.fspath(path))
