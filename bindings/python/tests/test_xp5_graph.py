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
    """An XP5 body with two buttons on one ADC channel; the guest loops reading it.

    The bands and samples are synthetic test inputs, not body facts.
    """
    # ldr w0, [x1, #0x14] (ADC channel 1); str w0, [x2]; b back to the load.
    rom = struct.pack("<3I", 0xB9401420, 0xB9000040, 0x17FFFFFE) + bytes(0x4000 - 12)
    buttons = [("menu_ok_button", 150), ("focus_stick", 350)]

    def level(value: int) -> dict[str, object]:
        return dict(kind="analog", node=f"{ADC:#x}", channel=1, value=value)

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
            controls=dict(
                wiring=dict(
                    controls=[
                        dict(
                            id=control,
                            label=control,
                            kind="button",
                            positions=["released", "pressed"],
                            initial="released",
                            unavailable=None,
                        )
                        for control, _ in buttons
                    ],
                    circuits=[
                        dict(
                            controls=[control for control, _ in buttons],
                            cases=[
                                dict(
                                    positions=["released", "released"],
                                    signals=[level(LADDER_RELEASED)],
                                ),
                                *(
                                    dict(
                                        positions=[
                                            "pressed" if other == control else "released"
                                            for other, _ in buttons
                                        ],
                                        signals=[level(sample)],
                                    )
                                    for control, sample in buttons
                                ),
                            ],
                        )
                    ],
                ),
                confirmation=dict(
                    poll_ticks=1,
                    max_phase_ticks=1000,
                    catalog=[
                        dict(id=control, label=control, kind="button", supported=True)
                        for control, _ in buttons
                    ],
                    actuators=[
                        dict(
                            id=control,
                            momentary=True,
                            states=[
                                dict(position=position, ready=[], expected=[], hold_ticks=1)
                                for position in ("released", "pressed")
                            ],
                        )
                        for control, _ in buttons
                    ],
                ),
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


def test_xp5_panel_presses_reach_the_adc_the_guest_reads() -> None:
    definition, assets = ladder_scenario()
    with make_board(definition, assets) as board:
        session = board.session

        def sample() -> int:
            board.run(64)
            assert board.failure is None
            return struct.unpack("<I", board.read_memory(0x11000, 4))[0]

        assert sample() == LADDER_RELEASED
        session.press("focus_stick")
        assert sample() == 350
        session.release("focus_stick")
        session.press("menu_ok_button")
        assert sample() == 150
        with pytest.raises(_native.emulator.NativeError, match="unavailable"):
            session.press("disp_back_button")
