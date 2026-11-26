"""The function watcher can count execution in raw PC intervals."""

from typing import cast

import pytest

from .test_watch_continuation import board


def test_instruction_ranges_are_opt_in_and_count_the_same_window() -> None:
    with board(1) as machine:
        native = machine.native
        native.function_watch_begin([0x10000])
        machine.run(6)
        state = native.function_watch_state()
        assert state is not None and "instruction_ranges" not in state
        native.function_watch_end()
        machine.cpus[0].reg_write("Pc", 0x10000)
        ranges = [(0x10000, 0x50000), (0x10000, 0x10004), (0x10004, 0x1000C)]
        ranges += [(0x100000 + 4 * i, 0x100004 + 4 * i) for i in range(10_000)]
        native.function_watch_begin([0x10000], instruction_ranges=ranges)
        machine.run(6)
        state = native.function_watch_state()
        assert state is not None
        assert state["hits"] == [(0x10000, 2)]
        instruction_ranges = cast(
            list[tuple[int, int, list[int]]], state["instruction_ranges"]
        )
        assert instruction_ranges[:3] == [
            (0x10000, 0x50000, [6]),
            (0x10000, 0x10004, [2]),
            (0x10004, 0x1000C, [4]),
        ]
        assert len(instruction_ranges) == len(ranges)
        native.function_watch_end()
        with pytest.raises((ValueError, RuntimeError)):
            native.function_watch_begin([0x10000], instruction_ranges=[(1, 1)])
