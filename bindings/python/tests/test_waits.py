"""Read-set waits follow dependencies and preserve hard execution ceilings."""
from dataclasses import asdict
import json

import pytest

from fujimod.emulator.waits import (
    MAX_CONTINUATION_STOP_CAUSES,
    ContinuationStopCause,
    ContinuationStopCount,
    WaitMetrics,
    wait_steps,
    wait_until,
)


class Reads:
    def __init__(self):
        self.active = False
        self.stores = []
        self.devices = []
        self.stop = None
        self.stop_calls = 0

    def read_set_begin(self):
        assert not self.active
        self.active = True
        self.stores, self.devices = [], []

    def read_set_end(self):
        assert self.active
        self.active = False
        return json.dumps(dict(stores=self.stores, devices=self.devices))

    def continuation_stop(self):
        self.stop_calls += 1
        return self.stop


class Board:
    def __init__(self):
        self.native = Reads()
        self.instructions = self.virtual_ticks = 0
        self.calls = []
        self.stop_events = []

    def read(self, address):
        if self.native.active:
            self.native.stores.append((address, address+4))
        return self.instructions

    def device(self, name):
        if self.native.active:
            self.native.devices.append(name)
        return self.instructions

    def run(self, limit, *, pause_at_tick=None, store_watches=None, device_watches=None):
        self.calls.append((limit, pause_at_tick, store_watches, device_watches))
        count = min(limit, 3 if store_watches or device_watches else limit)
        if pause_at_tick is not None:
            count = min(count, pause_at_tick-self.virtual_ticks)
        self.instructions += count
        self.virtual_ticks += count
        self.native.stop = self.stop_events.pop(0) if self.stop_events else None
        return self.native.stop is not None


def test_read_set_changes_after_short_circuit_and_tracks_device_reads():
    board = Board()
    report = WaitMetrics('dependent')
    assert wait_until(board, lambda: board.read(8) >= 3 and board.device('timer') >= 6,
                      'dependent', instruction_limit=20, metrics=report)
    assert board.calls == [(20, None, [(8,12)], []), (17, None, [(8,12)], ['timer'])]
    assert (report.polls, report.continuations, report.instructions, report.ticks) == (3,2,6,6)
    assert board.native.stop_calls == 0


def test_already_ready_never_runs_and_unwatchable_false_predicate_refuses():
    board = Board()
    assert wait_until(board, lambda: 'ready', 'ready') == 'ready'
    with pytest.raises(ValueError, match='nothing watchable'):
        wait_until(board, lambda: False, 'no input')
    assert board.calls == []


def test_pure_tick_wait_uses_its_named_tick_and_hard_deadline():
    board = Board()
    wait_until(board, lambda: board.virtual_ticks >= 8, 'tick', until_tick=8, deadline=10)
    assert board.virtual_ticks == 8
    assert len(board.calls) == 1
    with pytest.raises(TimeoutError):
        wait_until(board, lambda: board.virtual_ticks >= 20, 'late', until_tick=20, deadline=10)
    assert board.virtual_ticks == 10


def test_watch_wakes_do_not_replenish_the_instruction_allowance():
    board = Board()
    with pytest.raises(TimeoutError):
        wait_until(board, lambda: board.read(4) >= 30, 'late', instruction_limit=8)
    assert board.instructions == 8
    assert [row[0] for row in board.calls] == [8,5,2]


def test_exception_closes_read_tracking_and_generator_close_cancels():
    board = Board()
    def broken():
        board.read(4)
        raise LookupError('predicate error')
    with pytest.raises(LookupError):
        wait_until(board, broken, 'broken')
    assert not board.native.active
    operation = wait_steps(board, lambda: board.read(4) >= 100, 'cancel')
    next(operation)
    operation.close()
    assert board.instructions == 3
    assert not board.native.active


def test_quiescent_false_wait_refuses_instead_of_spinning():
    board = Board()
    board.run = lambda *args, **kwargs: None
    with pytest.raises(RuntimeError, match='no progress'):
        wait_until(board, lambda: board.read(4) > 10, 'quiescent')


def test_device_publications_at_the_current_tick_are_progress():
    board = Board()
    publications = 0

    def publish(*args, **kwargs):
        nonlocal publications
        publications += 1
        return True

    def ready():
        board.device('sink')
        return publications == 2

    board.run = publish
    assert wait_until(board, ready, 'sink', instruction_limit=1, deadline=1)
    assert publications == 2
    assert (board.instructions, board.virtual_ticks) == (0, 0)
    # A previous publication must not hide a subsequent quiescent return.
    board.run = lambda *args, **kwargs: False
    with pytest.raises(RuntimeError, match='no progress'):
        wait_until(board, lambda: board.device('sink'), 'quiescent sink')


def test_an_explicit_continuation_ceiling_does_not_replace_the_total_allowance():
    board = Board()
    wait_until(board, lambda: board.read(4) >= 3, 'bounded',
               instruction_limit=8, max_run_instructions=1)
    assert [row[0] for row in board.calls] == [1, 1, 1]
    assert board.instructions == 3
    with pytest.raises(ValueError, match='ceiling'):
        wait_until(board, lambda: False, 'invalid', max_run_instructions=0)


def test_ignored_device_wakes_require_and_use_bounded_continuations():
    board = Board()
    assert wait_until(board, lambda: board.device('sd-host') >= 7, 'card state',
                      max_run_instructions=3, ignored_devices=('sd-host',))
    assert board.calls == [(3, None, [], []), (3, None, [], []), (3, None, [], [])]
    with pytest.raises(ValueError, match='nothing watchable'):
        wait_until(Board(), lambda: False, 'unbounded', ignored_devices=('sd-host',))


def test_stop_profile_aggregates_causes_per_wait():
    board = Board()
    store_stop = {
        'kind': 'store',
        'range_index': 0,
        'range': [8, 12],
        'address': 8,
        'change_confirmed': True,
    }
    board.stop_events = [store_stop, {'kind': 'device', 'id': 2}, store_stop]
    report = WaitMetrics('profiled')

    assert wait_until(board, lambda: board.read(8) >= 9, 'profiled',
                      instruction_limit=20, metrics=report, profile_stops=True)

    assert report.continuation_stops == [
        ContinuationStopCount(
            ContinuationStopCause(
                'store', range_index=0, physical_range=(8, 12), address=8,
                change_confirmed=True,
            ),
            2,
        ),
        ContinuationStopCount(ContinuationStopCause('device', device_id=2), 1),
    ]
    assert report.continuation_stop_overflow == 0
    assert board.native.stop_calls == 3


def test_stop_profile_bounds_distinct_causes_and_counts_overflow():
    board = Board()
    event_count = MAX_CONTINUATION_STOP_CAUSES + 3
    board.stop_events = [{'kind': 'device', 'id': index} for index in range(event_count)]
    board.stop_events[-1] = {'kind': 'device', 'id': 0}
    report = WaitMetrics('bounded profile')

    assert wait_until(board, lambda: board.device('timer') >= event_count * 3,
                      'bounded profile', instruction_limit=event_count * 3 + 1,
                      metrics=report, profile_stops=True)

    assert len(report.continuation_stops) == MAX_CONTINUATION_STOP_CAUSES
    assert report.continuation_stops[0] == ContinuationStopCount(
        ContinuationStopCause('device', device_id=0), 2)
    assert report.continuation_stop_overflow == 2
    assert sum(item.count for item in report.continuation_stops) + report.continuation_stop_overflow == event_count


def test_profiled_wait_metrics_are_json_serializable():
    board = Board()
    board.stop_events = [{'kind': 'device', 'id': 4}]
    report = WaitMetrics('serializable')

    assert wait_until(board, lambda: board.device('timer') >= 3, 'serializable',
                      instruction_limit=10, metrics=report, profile_stops=True)

    serialized = json.dumps(asdict(report))
    decoded = json.loads(serialized)
    assert decoded['continuation_stops'] == [
        {'cause': {'kind': 'device', 'device_id': 4, 'range_index': None,
                   'physical_range': None, 'address': None, 'change_confirmed': None},
         'count': 1}
    ]
    assert decoded['continuation_stop_overflow'] == 0


def test_stop_profile_requires_a_report_and_native_observation_api():
    board = Board()
    with pytest.raises(ValueError, match='requires metrics'):
        wait_until(board, lambda: 'ready', 'missing report', profile_stops=True)
    board.native.continuation_stop = None
    with pytest.raises(ValueError, match='does not expose'):
        wait_until(board, lambda: 'ready', 'missing API', metrics=WaitMetrics('missing API'),
                   profile_stops=True)


def test_stop_profile_rejects_a_missing_cause_for_a_published_continuation():
    board = Board()
    board.run = lambda *args, **kwargs: True
    with pytest.raises(RuntimeError, match='without a cause'):
        wait_until(board, lambda: board.read(8) >= 100, 'missing cause',
                   metrics=WaitMetrics('missing cause'), profile_stops=True)
