"""XP5 construction and physical inputs through the ordinary native board owner."""

import struct

import pytest
from fujimod import _native
from fujimod.emulator.board import make_board
from fujimod.emulator.definition import Asset, Definition, Image


def execution(peripheral: int, registers: list[list[object]]) -> dict[str, object]:
    """One AArch64 core at EL3 running from a ROM at 0xFFF00000."""
    return dict(
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
                    *registers,
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
            dict(address=0xFFF00000, physical=0xFFF00000, size=0x4000, permissions=1),
        ],
        decoders=[],
        peripherals=[dict(core=0, address=peripheral, physical=peripheral, size=4096)],
        instructions_per_tick=[1, 1],
    )


def scenario() -> tuple[Definition, dict[str, bytes]]:
    rom = struct.pack("<3I", 0xD5384240, 0xB9000020, 0x14000000) + bytes(0x4000 - 12)
    return Definition(
        execution=execution(0xFF90B000, [[{"X": 1}, 0x11000]]),
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
        assets=(Asset("rom", len(rom)),),
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


ADC = 0xFF906000
LADDER_RELEASED = 0x456


def ladder_scenario() -> tuple[Definition, dict[str, bytes]]:
    """An XP5 body whose keys are an ADC ladder; the guest loops reading it.

    The bands and samples are synthetic test inputs, not body facts.
    """
    # ldr w0, [x1, #0x14] (ADC channel 1); str w0, [x2]; b back to the load.
    rom = struct.pack("<3I", 0xB9401420, 0xB9000040, 0x17FFFFFE) + bytes(0x4000 - 12)
    rungs = [("Menu", 100, 200, 150), ("StickPush", 300, 400, 350)]
    return Definition(
        execution=execution(ADC, [[{"X": 1}, ADC], [{"X": 2}, 0x11000]]),
        devices=dict(
            kind="xp5",
            boot_entry="rom",
            windows=[
                dict(
                    kind="adc",
                    base=ADC,
                    size=4096,
                    options=dict(samples=[LADDER_RELEASED] * 15),
                )
            ],
            ladder_keys=dict(
                ladders=[
                    dict(
                        wire=dict(kind="adc", channel=1),
                        levels=4096,
                        rungs=[
                            dict(
                                key=key,
                                band_low=low,
                                band_high=high,
                                sample=sample,
                                basis="test fixture",
                            )
                            for key, low, high, sample in rungs
                        ],
                    )
                ]
            ),
            storage=None,
            companion=None,
            displays=[],
            interrupts=None,
            aprofile=[],
        ),
        assets=(Asset("rom", len(rom)),),
        images=(Image("rom", 0xFFF00000),),
    ), {"rom": rom}


def test_xp5_ladder_keys_reach_the_adc_the_guest_reads() -> None:
    definition, assets = ladder_scenario()
    with make_board(definition, assets) as board:
        session = board.session

        def sample() -> int:
            board.run(64)
            assert board.failure is None
            return struct.unpack("<I", board.read_memory(0x11000, 4))[0]

        assert sample() == LADDER_RELEASED
        session.xp5_key(board.virtual_ticks, "STICK_PUSH", True)
        assert sample() == 350
        session.xp5_key(board.virtual_ticks, "STICK_PUSH", False)
        session.xp5_key(board.virtual_ticks, "MENU", True)
        assert sample() == 150
        with pytest.raises(
            _native.emulator.NativeError, match="no declared ladder rung"
        ):
            session.xp5_key(board.virtual_ticks, "DISP/BACK", True)
