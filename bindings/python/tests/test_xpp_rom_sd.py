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


def options() -> dict[str, object]:
    # GFX 50R wiring from the Agate ROM-to-card-loader execution record.
    return dict(
        ram_size=RAM,
        cores=2,
        discovery=True,
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


@pytest.mark.skipif(
    not (ROM and CARD and TERMINAL),
    reason="needs FUJIMOD_XPP_ROM, FUJIMOD_XPP_CARD and FUJIMOD_XPP_TERMINAL",
)
def test_rom_loads_card_image_and_reaches_its_terminal_loop() -> None:
    assert ROM and CARD
    files = [(name, (Path(CARD) / name).read_bytes()) for name in ("ffsys.bin", "ffboot.bin")]
    board = native.Board.from_xp4(json.dumps(options()), card_files=files)
    board.physical_write(0xFFF00000, Path(ROM).read_bytes()[:0x8000])
    board.watch_pcs([0, TERMINAL])
    reached: set[tuple[int, int]] = set()
    while sum(board.instructions) < 10_000_000:
        board.run(100_000, 1000, None, None)
        assert json.loads(board.failure()) is None
        reached = {(row["core"], row["pc"]) for row in json.loads(board.pc_entries())}
        if (0, TERMINAL) in reached:
            break
    assert (0, 0) in reached and (0, TERMINAL) in reached
    ffboot = files[1][1]
    assert bytes(board.physical_read(0, len(ffboot))) == ffboot
