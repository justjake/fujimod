"""Checked AArch64 routines reaching a declared XP5 device set.

The code, flash image and register sequences are synthetic test inputs.
"""

import struct

import pytest
from fujimod.emulator.aarch64_harness import (
    Aarch64Asset,
    Aarch64Devices,
    Aarch64DriverHarness,
    Aarch64Image,
    Aarch64Inputs,
)
from fujimod.emulator.driver_harness import DriverHarness

BASE = 0x1_0000_0000
CODE, DATA, SP, RETURN = BASE + 0x1000, BASE + 0x4000, BASE + 0x9000, BASE + 0xB000
NOR, DMA, CLOCKS = 0xFF90A000, 0xFF939000, 0xFFA01000
JEDEC = 0x20BB20
FLASH = bytes(range(256)) * 16


def words(*values: int) -> bytes:
    return struct.pack("<" + "I" * len(values), *values)


def devices(windows: tuple[tuple[int, int], ...]) -> Aarch64Devices:
    return Aarch64Devices(
        definition=dict(
            kind="xp5",
            boot_entry="rom",
            windows=[
                dict(
                    kind="flash-storage",
                    base=0xF0000000,
                    size=0x08000000,
                    options=dict(erased_tail=255, medium="nor"),
                ),
                dict(
                    kind="flash-storage-controller",
                    base=NOR,
                    size=0x1000,
                    options=dict(jedec_id=JEDEC),
                ),
                dict(kind="flash-storage-dma", base=DMA, size=0x1000, options={}),
                dict(kind="clock-banks", base=0xFFA00000, size=0x20000, options={}),
            ],
            storage="flash",
            companion=None,
            displays=[],
            interrupts=None,
            aprofile=[],
        ),
        windows=windows,
        assets=(Aarch64Asset("flash", FLASH),),
    )


def harness(code: bytes, windows: tuple[tuple[int, int], ...]) -> Aarch64DriverHarness:
    inputs = Aarch64Inputs(
        images=(Aarch64Image("code", CODE, code),),
        stack=(SP - 0x1000, SP + 64),
        stack_pointer=SP,
        return_address=RETURN,
        scratch=(BASE + 0xC000, BASE + 0x10000),
        devices=devices(windows),
    )
    result = DriverHarness(inputs, ram_fixture=[(DATA, bytes(32))])
    assert isinstance(result, Aarch64DriverHarness)
    return result


def jedec_routine() -> bytes:
    """x0 = NOR controller, x1 = output buffer, x2 = clock bank."""
    return words(
        0x52800024,  # mov w4, #1
        0xB9000004,  # str w4, [x0]          single-byte command field
        0x528013E4,  # mov w4, #0x9f
        0xB9004004,  # str w4, [x0, #0x40]   command
        0x52800044,  # mov w4, #2
        0xB900A004,  # str w4, [x0, #0xa0]   three bytes
        0x52800024,  # mov w4, #1
        0xB900C004,  # str w4, [x0, #0xc0]   begin
        0xB940F003,  # ldr w3, [x0, #0xf0]   status
        0xB9000023,  # str w3, [x1]
        0xB9408003,  # ldr w3, [x0, #0x80]
        0x39001023,  # strb w3, [x1, #4]
        0xB9408003,  # ldr w3, [x0, #0x80]
        0x39001423,  # strb w3, [x1, #5]
        0xB9408003,  # ldr w3, [x0, #0x80]
        0x39001823,  # strb w3, [x1, #6]
        0x52800244,  # mov w4, #0x12
        0xB9004044,  # str w4, [x2, #0x40]   clock divider 0
        0xB9404045,  # ldr w5, [x2, #0x40]
        0xB9000825,  # str w5, [x1, #8]
        0xD65F03C0,  # ret
    )


def test_routine_reads_the_declared_nor_controller_jedec_id() -> None:
    windows = ((NOR, NOR + 0x1000), (CLOCKS, CLOCKS + 0x1000))
    with harness(jedec_routine(), windows) as h:
        result = h.call(CODE, args=(NOR, DATA, CLOCKS), max_instructions=100)
        assert result["failure"] is None and result["returned"]
        status, jedec, divider = struct.unpack("<I3sxI", h.read(DATA, 12))
        assert jedec == JEDEC.to_bytes(3, "big")
        assert status == 7  # bit 2 set, receive pending, complete
        assert divider == 0x12
        rows = result["device_transactions"]
        assert [(r["address"], r["operation"]) for r in rows][:2] == [
            (NOR, "write"),
            (NOR + 0x40, "write"),
        ]
        assert [r["value"] for r in rows if r["address"] == NOR + 0x80] == [
            0x20,
            0xBB,
            0x20,
        ]
        assert result["device_transactions_omitted"] == 0
        assert result["caller"]["devices"] == [list(w) for w in windows]
        assert result["devices"]["kind"] == "xp5"
        assert result["device_assets"][0]["name"] == "flash"
        assert result["device_assets"][0]["size"] == len(FLASH)
        assert result["execution"]["peripherals"] == [
            {"core": 0, "address": a, "physical": a, "size": b - a} for a, b in windows
        ]


def test_undeclared_device_window_refuses_before_execution() -> None:
    # The DMA exists in the device set but is not a routine window.
    code = words(0xB9400003, 0xD65F03C0)  # ldr w3, [x0]; ret
    with harness(code, ((NOR, NOR + 0x1000),)) as h:
        result = h.call(CODE, args=(DMA,), max_instructions=10)
        assert not result["returned"]
        assert "read outside declared range" in result["failure"]
        assert result["device_transactions"] == []


def test_unmodeled_register_inside_a_declared_window_refuses() -> None:
    code = words(0xB9400C03, 0xD65F03C0)  # ldr w3, [x0, #0xc]; ret
    with harness(code, ((NOR, NOR + 0x1000),)) as h:
        result = h.call(CODE, args=(NOR,), max_instructions=10)
        assert not result["returned"]
        assert "unmodeled MMIO read4 at 0xff90a00c" in result["failure"]


def test_device_windows_must_not_overlap_caller_memory() -> None:
    with pytest.raises(ValueError):
        Aarch64Inputs(
            images=(Aarch64Image("code", CODE, words(0xD65F03C0)),),
            stack=(SP - 0x1000, SP + 64),
            stack_pointer=SP,
            return_address=RETURN,
            scratch=(BASE + 0xC000, BASE + 0x10000),
            devices=devices(((CODE, CODE + 0x1000),)),
        )
