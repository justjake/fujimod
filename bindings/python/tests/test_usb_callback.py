"""Caller-supplied callback plans and explicit reclaim through the Rust core."""

import struct

import pytest
from fujimod.usb import research, testing

from fujimod import usb

LIMITS = usb.FirmwareLimits(memory_read=64, memory_write=64, configuration=1, console=1)
E = testing.Exchange


def control(op: int, size: int, sid: int = 42) -> testing.Exchange:
    return E.control(struct.pack(">BBIIH", op, 0, 0, size, sid))


def command(request: bytes, reply: bytes) -> list[testing.Exchange]:
    return [
        control(0xD0, len(request)),
        E.write(request),
        control(0xD1, len(reply)),
        E.read(len(reply), reply),
    ]


def read(address: int, value: bytes) -> list[testing.Exchange]:
    request = struct.pack("<9I", 0x200001, 0, 0, 0, 2, 0, 0, address, len(value))
    return command(request, request + value)


def write(address: int, value: bytes) -> list[testing.Exchange]:
    header = struct.pack("<9I", 0x200001, 0, 0, 0, 3, 0, 0, address, len(value))
    return command(header + value, header) + read(address, value)


async def test_reclaim_then_open_is_explicit() -> None:
    connection, script = testing.connection([
        control(0x1D, 0, 0),
        E.read(2, b"\x29\0"),
        control(0xC0, 8, 0),
        E.read(8, b"\x2a\0\0\0\0\0\0\x40"),
    ])
    assert isinstance(connection, usb.Connection)
    assert await connection.reclaim_session() is True
    session = await connection.begin_session(LIMITS)
    assert session.token == 42 and script.remaining == 0


@pytest.mark.parametrize("expect_reply", [False, True])
async def test_callback_reply_restores_slot(expect_reply: bool) -> None:
    raw = struct.pack("<8I", 0x450006, 0, 0, 0, 0, 0, 0, 0)
    request = research.Request(
        research.CommandCode(0x450006), bytes(16), reply_capacity=32
    )
    invocation = usb.CallbackInvocation(
        0x100,
        bytes(8),
        b"\x09" * 8,
        request,
        LIMITS,
        replies=[raw] if expect_reply else None,
    )
    invocation.require(0x200, 1, [b"\x01", b"\x02"])
    invocation.write(0x300, [b"\0"], b"\x03")
    invocation.synchronize(usb.CommandSequence())
    steps = [control(0xC0, 8, 0), E.read(8, b"\x2a\0\0\0\0\0\0\x40")]
    steps += read(0x200, b"\x02") + read(0x300, b"\0") + read(0x100, bytes(8))
    steps += read(0x300, b"\0") + write(0x300, b"\x03")
    steps += read(0x100, bytes(8)) + write(0x100, b"\x09" * 8)
    steps += command(raw, raw) + read(0x100, b"\x09" * 8) + write(0x100, bytes(8))
    session, script = await testing.session(steps, LIMITS)
    if expect_reply:
        assert await session.invoke_callback(invocation) == raw
    else:
        with pytest.raises(usb.UsbError, match="callback invocation failed") as failure:
            await session.invoke_callback(invocation)
        assert failure.value.restoration_error is None
    assert script.remaining == 0 and session.state == usb.SessionState.Ready
