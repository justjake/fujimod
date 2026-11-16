"""XP4 NOR observation exposes completed media without changing execution."""

import json

import pytest
from fujimod._native.emulator import NativeError
from fujimod.emulator import Asset, Xp4Definition, make_board


def test_nor_media_and_observation_are_available_without_running() -> None:
    medium = bytes(range(256)) * 16
    board = make_board(
        Xp4Definition(
            options={
                "ram_size": 65536,
                "cores": 1,
                "discovery": False,
                "cbar": 0xFFFF0000,
                "storage_jedec": 0x20,
                "storage_geometry": [4096, 256, 8, 2],
                "storage_irq": 37,
                "storage_dma_routes": list(range(207, 223)),
            },
            assets=(Asset("nor", len(medium)),),
            storage="nor",
        ),
        {"nor": medium},
    )
    try:
        xp4 = board.native.xp4
        before = xp4.state(False)
        flash = json.loads(xp4.device_state_json()["xp4-serial-flash"])
        assert flash["nor"]["program_count"] == 0
        assert flash["nor"]["programmed_ranges"] == []
        assert xp4.nor_medium() == medium
        xp4.reset_nor_program_coverage()
        assert xp4.nor_medium() == medium
        assert xp4.state(False) == before
    finally:
        board.close()


def test_nor_observation_refuses_board_without_nor() -> None:
    board = make_board(
        Xp4Definition(
            options={
                "ram_size": 65536,
                "cores": 1,
                "discovery": False,
                "cbar": 0xFFFF0000,
            }
        ),
        {},
    )
    try:
        with pytest.raises(NativeError, match="no (devices|XP4 NOR)"):
            board.native.xp4.nor_medium()
        with pytest.raises(NativeError, match="no (devices|XP4 NOR)"):
            board.native.xp4.reset_nor_program_coverage()
    finally:
        board.close()
