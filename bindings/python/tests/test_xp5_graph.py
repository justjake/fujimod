"""XP5 construction and physical inputs through the ordinary native board owner."""

import hashlib
import struct

import pytest
from fujimod import _native
from fujimod.emulator.board import make_board
from fujimod.emulator.definition import Asset, Definition, Image


def scenario() -> tuple[Definition, dict[str, bytes]]:
    rom = struct.pack("<3I", 0xD5384240, 0xB9000020, 0x14000000) + bytes(0x4000 - 12)
    return Definition(
        execution=dict(
            processors=[
                dict(
                    model="armv8a",
                    registers=[
                        ["Pc", 0xFFF00000],
                        ["Pstate", 0x3CD],
                        [
                            {"Arm64System": dict(op0=3, op1=6, crn=1, crm=1, op2=0)},
                            0x401,
                        ],
                        [
                            {"Arm64System": dict(op0=3, op1=4, crn=1, crm=1, op2=0)},
                            0x80000000,
                        ],
                        [{"X": 1}, 0x11000],
                    ],
                    released=True,
                    mprofile=None,
                    rprofile=None,
                )
            ],
            ram=[dict(base=0x10000, size=0x10000), dict(base=0xFFF00000, size=0x4000)],
            mappings=[
                dict(
                    core=0,
                    address=0x10000,
                    physical=0x10000,
                    size=0x10000,
                    permissions=7,
                ),
                dict(
                    core=0,
                    address=0xFFF00000,
                    physical=0xFFF00000,
                    size=0x4000,
                    permissions=5,
                ),
            ],
            dma_mappings=[
                dict(address=0x10000, physical=0x10000, size=0x10000, permissions=3),
                dict(
                    address=0xFFF00000, physical=0xFFF00000, size=0x4000, permissions=1
                ),
            ],
            decoders=[],
            peripherals=[
                dict(core=0, address=0xFF90B000, physical=0xFF90B000, size=4096)
            ],
            instructions_per_tick=[1, 1],
        ),
        devices=dict(
            kind="xp5",
            boot_entry="rom",
            windows=[dict(kind="gpio", base=0xFF90B000, size=4096, options={})],
            controls=dict(
                layout=dict(
                    shutter_half=dict(port=0, pin=0),
                    shutter_full=dict(port=0, pin=1),
                    rear_dial=[dict(port=1, pin=0), dict(port=1, pin=1)],
                    front_dial=[dict(port=2, pin=0), dict(port=2, pin=1)],
                ),
                initial_levels=[
                    dict(pin=dict(port=port, pin=pin), high=high)
                    for port, pin, high in [
                        (0, 0, True),
                        (0, 1, True),
                        (1, 0, False),
                        (1, 1, False),
                        (2, 0, False),
                        (2, 1, False),
                    ]
                ],
                samples=dict(
                    menu=100,
                    back=200,
                    menu_back=150,
                    up=300,
                    down=400,
                    left=500,
                    right=600,
                ),
                detent_return_ns=100,
                shutter_half_delay_ns=100,
            ),
            storage=None,
            companion=None,
            displays=[],
            interrupts=None,
            aprofile=[],
        ),
        assets=(Asset("rom", len(rom), hashlib.sha256(rom).hexdigest()),),
        images=(Image("rom", 0xFFF00000),),
    ), {"rom": rom}


def test_xp5_definition_executes_at_el3_and_drives_physical_inputs() -> None:
    definition, assets = scenario()
    with make_board(definition, assets) as board:
        session = board.session
        session.xp5_gpio_input(0, 7, True)
        session.xp5_key(0, "DISP/BACK", True)
        board.run(4)
        assert board.failure is None
        assert board.read_memory(0x11000, 4) == struct.pack("<I", 12)
        assert session.xp5_gpio_level(0, 7)
        session.xp5_gpio_input(0, 7, False)
        assert not session.xp5_gpio_level(0, 7)
        with pytest.raises(
            _native.emulator.NativeError, match="outside the declared pin range"
        ):
            session.xp5_gpio_input(20, 0, True)
        with pytest.raises(TypeError):
            session.xp5_gpio_input(True, 0, True)
