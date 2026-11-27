"""Guide boundaries work for boards with and without CPU exit counters."""

from fujimod import perf


class HardwareBoard:
    """A memory reader: no native core, so no CPU counters."""

    def read_memory(self, address: int, size: int) -> bytes:
        return bytes(size)


class EmulatedBoard:
    instructions = 7
    virtual_ticks = 3

    class native:
        @staticmethod
        def exit_counters():
            return {}


def test_end_guide_accepts_board_without_native_counters() -> None:
    perf.begin_guide("hardware-board")
    summary = perf.end_guide(HardwareBoard())
    assert isinstance(summary, dict)


def test_snapshot_applies_counters_only_to_boards_that_have_them(monkeypatch) -> None:
    calls = []
    monkeypatch.setattr(perf._native, "set_cpu_counters", calls.append)
    perf.snapshot_counters(HardwareBoard())
    assert calls == []
    perf.snapshot_counters(EmulatedBoard())
    assert len(calls) == 1
