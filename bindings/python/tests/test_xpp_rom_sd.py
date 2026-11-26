"""X-Processor Pro ROM-to-SD start on the XP4 board, from local ROM and card inputs."""

import json
import os
from pathlib import Path

import pytest
from fujimod._native import emulator as native

ROM = os.environ.get("FUJIMOD_XPP_ROM")
CARD = os.environ.get("FUJIMOD_XPP_CARD")
TERMINAL = int(os.environ.get("FUJIMOD_XPP_TERMINAL", "0"), 0)
RAM = 0x20000000


def options(terminal: int) -> dict[str, object]:
    # GFX 50R wiring from the Agate ROM-to-card-loader execution record.
    return dict(
        ram_size=RAM,
        cores=2,
        discovery=True,
        labels=[0, terminal],
        memory_regions=[
            dict(base=0xFFF00000, size=0x8000, permissions=5),
            dict(base=0xFFF0D000, size=0x1000, permissions=7),
        ],
        initial_registers=[[[{"Arm32": {"R": 15}}, 0xFFF00000]]] * 2,
        dram_alias=0x40000000,
        pmu_cycles_per_tick=64,
        xpp_reset_gates=0xFFF0A000,
        xpp_power=0xFFF10000,
        xpp_dram=(0xFFF0E000, 0),
        xpp_boot=dict(
            card_strap=True,
            low_alias=0xFFF00000,
            options=dict(
                selector_base=0xFFF0A000,
                clock_base=0xFFF10000,
                ddr_base=0xFFF0E000,
                remap_base=0xFF000000,
                bus_bases=list(range(0xFF242000, 0xFF249000, 0x1000)),
                memory_port_bases=[0xFF142000, 0xFF143000, 0xFF147000, 0xFF148000],
                decoder=0,
                normal_selector=0x18,
                card_selector=0,
                settle_ticks=2,
                refresh_level=0,
            ),
        ),
        xpp_sd=dict(
            options=dict(
                base=0xFFF92000,
                register_offset=0x200,
                transfer_ticks=16,
                clock_ticks=2,
                reset_ticks=2,
                rca=1,
                ram_bytes=RAM,
            ),
            card=dict(
                sectors=0x800000,
                max_dirty_sectors=524288,
                partition_start=2048,
                sectors_per_cluster=64,
                label="RECOVERY",
                read_only_files=[],
            ),
        ),
    )


def card_inputs(directory: str) -> list[tuple[str, bytes]]:
    return [
        (name, (Path(directory) / name).read_bytes())
        for name in ("ffsys.bin", "ffboot.bin")
    ]


def boot_board(
    rom: str, files: list[tuple[str, bytes]], terminal: int, *, strict: bool = False
) -> native.Board:
    config = options(terminal)
    if strict:
        # The shared XPP peripheral window used during ROM startup. The SD
        # strap comes from the existing fixture, independently of GPIO levels.
        config.update(
            discovery=False, cbar=0xFFFF0000, gpio_base=0xFFF0C000, gpio=[0xFFFF] * 16
        )
    board = native.Board.from_xp4(json.dumps(config), card_files=files)
    board.xp4.write(0xFFF00000, Path(rom).read_bytes()[:0x8000])
    return board


@pytest.mark.skipif(
    not (ROM and CARD and TERMINAL),
    reason="needs FUJIMOD_XPP_ROM, FUJIMOD_XPP_CARD and FUJIMOD_XPP_TERMINAL",
)
def test_rom_loads_card_image_and_reaches_its_terminal_loop() -> None:
    assert ROM and CARD
    files = card_inputs(CARD)
    board = boot_board(ROM, files, TERMINAL)
    xp4 = board.xp4
    milestones: dict[tuple[int, int], int] = {}
    while sum(board.instructions) < 10_000_000:
        board.run(100_000, 1000)
        state = xp4.state()
        assert state["failure"] is None
        milestones = {(core, address): n for core, address, n in state["milestones"]}
        if (0, TERMINAL) in milestones:
            break
    assert (0, 0) in milestones and (0, TERMINAL) in milestones
    ffboot = files[1][1]
    assert bytes(xp4.read(0, len(ffboot))) == ffboot


@pytest.mark.skipif(
    not (ROM and CARD) or TERMINAL,
    reason="needs FUJIMOD_XPP_ROM and FUJIMOD_XPP_CARD for a firmware startup image",
)
def test_rom_loads_firmware_image_through_the_reset_vector() -> None:
    assert ROM and CARD
    files = card_inputs(CARD)
    ffboot = files[1][1]
    assert len(ffboot) >= 0x24
    reset_entry = int.from_bytes(ffboot[0x20:0x24], "little")
    assert 0 < reset_entry < len(ffboot) and reset_entry % 4 == 0
    board = boot_board(ROM, files, reset_entry, strict=True)
    milestones: dict[tuple[int, int], int] = {}
    while sum(board.instructions) < 30_000_000:
        board.run(50_000, 1000)
        state = board.xp4.state()
        assert state["failure"] is None
        milestones = {(core, address): n for core, address, n in state["milestones"]}
        if (0, reset_entry) in milestones:
            break
    handoff = milestones[(0, 0)]
    assert milestones[(0, reset_entry)] == handoff + 1

    # Replay to the reset-vector instruction, before the reset handler can
    # clear memory or modify any of the bytes we want to check.
    replay = boot_board(ROM, files, reset_entry, strict=True)
    replay.run(handoff, 1000)
    state = replay.xp4.state()
    assert state["failure"] is None
    assert (0, 0, handoff) in state["milestones"]
    assert all(address != reset_entry for _, address, _ in state["milestones"])
    assert bytes(replay.xp4.read(0, len(ffboot))) == ffboot
