"""Read-set-bound host waits. Predicates observe; only Board.run executes."""
from __future__ import annotations

from collections.abc import Callable, Collection, Generator
from dataclasses import dataclass, field
import json
from typing import Protocol, TypeVar

from .. import perf


class NativeReads(Protocol):
    def read_set_begin(self) -> None: ...
    def read_set_end(self) -> str: ...
    def continuation_stop(self) -> dict[str, object] | None: ...


class WaitBoard(Protocol):
    @property
    def native(self) -> NativeReads: ...
    @property
    def instructions(self) -> int: ...
    @property
    def virtual_ticks(self) -> int: ...
    def run(self, limit: int, *, pause_at_tick: int | None = None,
            store_watches: list[tuple[int, int]] | None = None,
            device_watches: list[str] | None = None,
            contact_watch: bool = False) -> bool: ...


WAIT_KINDS = (
    'debounce',
    'input_gate',
    'osd_frame',
    'rec_return',
    'card_load',
    'shutter',
    'other',
)


def wait_kind(description: str) -> str:
    """Classify a wait description into the ledger's fixed categories."""
    text = description.lower()
    if any(word in text for word in ('shutter', 'capture start')):
        return 'shutter'
    if any(word in text for word in ('debounce', 'physical', 'scanner', 'key level')):
        return 'debounce'
    if any(word in text for word in ('card', 'load', 'recipe', 'film resource')):
        return 'card_load'
    if any(word in text for word in ('shooting', 'rec return', 'initialized rec')):
        return 'rec_return'
    if any(word in text for word in ('frame', 'render', 'osd', 'display')):
        return 'osd_frame'
    if any(word in text for word in ('gate', 'input', 'parameter')):
        return 'input_gate'
    return 'other'


@dataclass
class WaitStops:
    """Explicitly scoped continuation-stop observation for one wait."""

    continuations: int = 0
    continuation_stops: list[ContinuationStopCount] = field(default_factory=list)
    continuation_stop_overflow: int = 0


@dataclass(frozen=True)
class ContinuationStopCause:
    kind: str
    device_id: int | None = None
    range_index: int | None = None
    physical_range: tuple[int, int] | None = None
    address: int | None = None
    change_confirmed: bool | None = None


@dataclass
class ContinuationStopCount:
    cause: ContinuationStopCause
    count: int


MAX_CONTINUATION_STOP_CAUSES = 32


def _continuation_stop_cause(value: object) -> ContinuationStopCause:
    if not isinstance(value, dict):
        raise RuntimeError('Native continuation stop returned an invalid record')
    if value.get('kind') == 'device':
        device_id = value.get('id')
        if type(device_id) is not int or device_id < 0:
            raise RuntimeError('Native continuation stop returned an invalid device id')
        return ContinuationStopCause('device', device_id=device_id)
    if value.get('kind') == 'store':
        range_index = value.get('range_index')
        physical_range = value.get('range')
        address = value.get('address')
        change_confirmed = value.get('change_confirmed')
        if (
            type(range_index) is not int
            or range_index < 0
            or not isinstance(physical_range, (list, tuple))
            or len(physical_range) != 2
            or any(type(part) is not int for part in physical_range)
            or physical_range[0] >= physical_range[1]
            or type(address) is not int
            or not physical_range[0] <= address < physical_range[1]
            or type(change_confirmed) is not bool
        ):
            raise RuntimeError('Native continuation stop returned an invalid store record')
        return ContinuationStopCause(
            'store',
            range_index=range_index,
            physical_range=(physical_range[0], physical_range[1]),
            address=address,
            change_confirmed=change_confirmed,
        )
    raise RuntimeError('Native continuation stop returned an unknown cause')


def _record_continuation_stop(report: WaitStops, value: object) -> None:
    cause = _continuation_stop_cause(value)
    for count in report.continuation_stops:
        if count.cause == cause:
            count.count += 1
            return
    if len(report.continuation_stops) < MAX_CONTINUATION_STOP_CAUSES:
        report.continuation_stops.append(ContinuationStopCount(cause, 1))
        return
    report.continuation_stop_overflow += 1


T = TypeVar('T')


def wait_steps(board: WaitBoard, predicate: Callable[[], T], description: str, *,
               instruction_limit: int = (1 << 63), deadline: int | None = None,
               until_tick: int | None = None,
               max_run_instructions: int | None = None,
               ignored_devices: Collection[str] = (),
               after_run: Callable[[], object] | None = None,
               stops: WaitStops | None = None,
               profile_stops: bool = False) -> Generator[None, None, T]:
    """Yield at passive watch stops, returning the predicate's truthy result.

    Short-circuited reads are rediscovered on every evaluation, so pointer and
    state-dependent predicates follow their current dependencies. A pure clock
    wait must supply until_tick; deadline is a failure ceiling, not a poll clock.
    instruction_limit is relative to this call and never replenished on a wake.
    profile_stops records stop causes in stops, with a fixed bound on distinct
    causes; use stops= to retain them.
    """
    if type(instruction_limit) is not int or instruction_limit < 0:
        raise ValueError('Wait instruction limit must be nonnegative')
    if until_tick is not None and (type(until_tick) is not int or until_tick < 0):
        raise ValueError('Wait tick must be a nonnegative integer')
    if max_run_instructions is not None and (type(max_run_instructions) is not int or max_run_instructions <= 0):
        raise ValueError('Wait continuation ceiling must be positive')
    if isinstance(ignored_devices, str) or any(not isinstance(name, str) or not name for name in ignored_devices):
        raise ValueError('Ignored device names must be a collection of nonempty strings')
    if type(profile_stops) is not bool:
        raise ValueError('Stop profiling must be enabled with a boolean')
    if profile_stops and stops is None:
        raise ValueError('Stop profiling requires stops=')
    if profile_stops and not callable(getattr(board.native, 'continuation_stop', None)):
        raise ValueError('Native board does not expose continuation stop observations')
    ignored_devices = frozenset(ignored_devices)
    start, tick = board.instructions, board.virtual_ticks
    report = stops if stops is not None else WaitStops()
    polls = 0
    properties = dict(instructions=0, ticks=0, polls=0, continuations=0)
    with perf.span('wait', wait_kind(description), properties):
      try:
        while True:
            board.native.read_set_begin()
            try:
                value = predicate()
            finally:
                reads = json.loads(board.native.read_set_end())
            polls += 1
            if value:
                return value
            remaining = instruction_limit - (board.instructions - start)
            if remaining <= 0 or (deadline is not None and board.virtual_ticks >= deadline):
                raise TimeoutError('Native execution did not complete: ' + description)
            stores = [(int(lo), int(hi)) for lo, hi in reads['stores']]
            devices = [str(name) for name in reads['devices'] if name not in ignored_devices]
            contacts = reads.get('contacts') is True
            watched = bool(stores or devices or contacts)
            if not watched and until_tick is None and max_run_instructions is None:
                raise ValueError('Wait predicate reads nothing watchable; name until_tick: ' + description)
            if not watched and until_tick is not None and board.virtual_ticks >= until_tick:
                raise ValueError('Wait predicate is false at its named tick: ' + description)
            boundaries = [t for t in (deadline, until_tick) if t is not None and t > board.virtual_ticks]
            boundary = min(boundaries) if boundaries else None
            before = board.instructions, board.virtual_ticks
            # Only a predicate that read the contact queue names its watch, so
            # boards without contact automation keep their run signature.
            published = board.run(remaining if max_run_instructions is None else min(remaining, max_run_instructions), pause_at_tick=boundary,
                                  store_watches=stores, device_watches=devices,
                                  **({'contact_watch': True} if contacts else {}))
            report.continuations += 1
            if profile_stops and published:
                stop = board.native.continuation_stop()
                if stop is None:
                    raise RuntimeError('Native run reported a continuation stop without a cause')
                _record_continuation_stop(report, stop)
            if after_run is not None:
                after_run()
            # A due device effect can publish at the current tick before any
            # CPU runs. Its watch is progress too; reevaluate the predicate.
            if not published and before == (board.instructions, board.virtual_ticks):
                raise RuntimeError('Native execution made no progress: ' + description)
            yield
      finally:
        properties.update(
            instructions=board.instructions - start,
            ticks=board.virtual_ticks - tick,
            polls=polls,
            continuations=report.continuations,
        )


def wait_until(board: WaitBoard, predicate: Callable[[], T], description: str, *,
               instruction_limit: int = (1 << 63), deadline: int | None = None,
               until_tick: int | None = None,
               max_run_instructions: int | None = None,
               ignored_devices: Collection[str] = (),
               after_run: Callable[[], object] | None = None,
               stops: WaitStops | None = None,
               profile_stops: bool = False) -> T:
    steps = wait_steps(board, predicate, description, instruction_limit=instruction_limit,
                       deadline=deadline, until_tick=until_tick, max_run_instructions=max_run_instructions,
                       ignored_devices=ignored_devices,
                       after_run=after_run, stops=stops, profile_stops=profile_stops)
    while True:
        try:
            next(steps)
        except StopIteration as done:
            return done.value
