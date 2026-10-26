"""Read-set-bound host waits. Predicates observe; only Board.run executes."""
from __future__ import annotations

from collections.abc import Callable, Generator
from dataclasses import dataclass
from contextlib import contextmanager
from contextvars import ContextVar
import json
from time import perf_counter
from typing import Protocol, TypeVar


class NativeReads(Protocol):
    def read_set_begin(self) -> None: ...
    def read_set_end(self) -> str: ...


class WaitBoard(Protocol):
    @property
    def native(self) -> NativeReads: ...
    @property
    def instructions(self) -> int: ...
    @property
    def virtual_ticks(self) -> int: ...
    def run(self, limit: int, *, pause_at_tick: int | None = None,
            store_watches: list[tuple[int, int]] | None = None,
            device_watches: list[str] | None = None) -> bool: ...


@dataclass
class WaitMetrics:
    description: str
    instructions: int = 0
    ticks: int = 0
    polls: int = 0
    continuations: int = 0
    wall_seconds: float = 0


_collector: ContextVar[list[WaitMetrics] | None] = ContextVar('wait_metrics', default=None)


@contextmanager
def collect_waits() -> Generator[list[WaitMetrics], None, None]:
    """Collect host measurements without affecting any guest decisions."""
    reports: list[WaitMetrics] = []
    token = _collector.set(reports)
    try:
        yield reports
    finally:
        _collector.reset(token)


T = TypeVar('T')


def wait_steps(board: WaitBoard, predicate: Callable[[], T], description: str, *,
               instruction_limit: int = (1 << 63), deadline: int | None = None,
               until_tick: int | None = None,
               max_run_instructions: int | None = None,
               after_run: Callable[[], object] | None = None,
               metrics: WaitMetrics | None = None) -> Generator[None, None, T]:
    """Yield at passive watch stops, returning the predicate's truthy result.

    Short-circuited reads are rediscovered on every evaluation, so pointer and
    state-dependent predicates follow their current dependencies. A pure clock
    wait must supply until_tick; deadline is a failure ceiling, not a poll clock.
    instruction_limit is relative to this call and never replenished on a wake.
    """
    if type(instruction_limit) is not int or instruction_limit < 0:
        raise ValueError('Wait instruction limit must be nonnegative')
    if until_tick is not None and (type(until_tick) is not int or until_tick < 0):
        raise ValueError('Wait tick must be a nonnegative integer')
    if max_run_instructions is not None and (type(max_run_instructions) is not int or max_run_instructions <= 0):
        raise ValueError('Wait continuation ceiling must be positive')
    start, tick, wall = board.instructions, board.virtual_ticks, perf_counter()
    report = metrics if metrics is not None else WaitMetrics(description)
    try:
        while True:
            board.native.read_set_begin()
            try:
                value = predicate()
            finally:
                reads = json.loads(board.native.read_set_end())
            report.polls += 1
            if value:
                return value
            remaining = instruction_limit - (board.instructions - start)
            if remaining <= 0 or (deadline is not None and board.virtual_ticks >= deadline):
                raise TimeoutError('Native execution did not complete: ' + description)
            stores = [(int(lo), int(hi)) for lo, hi in reads['stores']]
            devices = [str(name) for name in reads['devices']]
            if not stores and not devices and until_tick is None:
                raise ValueError('Wait predicate reads nothing watchable; name until_tick: ' + description)
            if not stores and not devices and until_tick is not None and board.virtual_ticks >= until_tick:
                raise ValueError('Wait predicate is false at its named tick: ' + description)
            boundaries = [t for t in (deadline, until_tick) if t is not None and t > board.virtual_ticks]
            boundary = min(boundaries) if boundaries else None
            before = board.instructions, board.virtual_ticks
            published = board.run(remaining if max_run_instructions is None else min(remaining, max_run_instructions), pause_at_tick=boundary,
                                  store_watches=stores, device_watches=devices)
            report.continuations += 1
            if after_run is not None:
                after_run()
            # A due device effect can publish at the current tick before any
            # CPU runs. Its watch is progress too; reevaluate the predicate.
            if not published and before == (board.instructions, board.virtual_ticks):
                raise RuntimeError('Native execution made no progress: ' + description)
            yield
    finally:
        report.instructions = board.instructions - start
        report.ticks = board.virtual_ticks - tick
        report.wall_seconds = perf_counter() - wall
        if (collector := _collector.get()) is not None:
            collector.append(report)


def wait_until(board: WaitBoard, predicate: Callable[[], T], description: str, *,
               instruction_limit: int = (1 << 63), deadline: int | None = None,
               until_tick: int | None = None,
               max_run_instructions: int | None = None,
               after_run: Callable[[], object] | None = None,
               metrics: WaitMetrics | None = None) -> T:
    steps = wait_steps(board, predicate, description, instruction_limit=instruction_limit,
                       deadline=deadline, until_tick=until_tick, max_run_instructions=max_run_instructions,
                       after_run=after_run, metrics=metrics)
    while True:
        try:
            next(steps)
        except StopIteration as done:
            return done.value
