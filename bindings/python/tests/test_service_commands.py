"""Application status and explicit unclassified requests over native framing."""

import struct
from collections.abc import Sequence

import pytest
from fujimod import usb
from fujimod.usb import app80, research, testing

LIMITS = usb.FirmwareLimits(memory_read=1, memory_write=1, configuration=1, console=1)


async def opened(
    exchanges: Sequence[testing.Exchange],
) -> tuple[usb.Session, testing.Script]:
    e = testing.Exchange
    return await testing.session(
        [
            e.control(struct.pack(">BBIIH", 0xC0, 0, 0, 8, 0)),
            e.read(8, b"\x34\x12\0\0\0\0\0\x01"),
            *exchanges,
        ],
        LIMITS,
    )


def command(request: bytes, capacity: int, reply: bytes) -> list[testing.Exchange]:
    e = testing.Exchange
    return [
        e.control(struct.pack(">BBIIH", 0xD0, 0, 0, len(request), 0x1234)),
        e.write(request),
        e.control(struct.pack(">BBIIH", 0xD1, 0, 0, capacity, 0x1234)),
        e.read(capacity, reply),
    ]


def info(selector: int, text: str) -> tuple[list[testing.Exchange], bytes]:
    request = struct.pack("<8I", 0x150000, 0, 0, 0, selector, 0, 0, 0)
    raw = struct.pack("<4I", 0x150000, 0, 0, 0) + (text + "\0").encode("utf-16le")
    return command(request, 128, raw), raw


async def test_download_writing_is_a_single_read_and_clear_is_explicit() -> None:
    read, raw = info(0xF0001, "WRITING")
    clear, _ = info(0xF0002, "NORMAL")
    session, script = await opened(read + clear)
    reply = await session.firmware_download_status()
    assert reply.state == usb.FirmwareDownloadState.Writing and reply.raw == raw
    assert reply.failure_code is None and reply.text == "WRITING"
    assert session.state == usb.SessionState.Ready and script.remaining == 4
    assert (
        await session.clear_firmware_download_status()
    ).state == usb.FirmwareDownloadState.Normal
    assert script.remaining == 0 and session.state == usb.SessionState.Ready


@pytest.mark.parametrize(
    "text,state,code",
    [
        ("FAIL00", usb.FirmwareDownloadState.Failed, 0),
        ("FAIL99", usb.FirmwareDownloadState.Failed, 99),
        ("FAIL1", usb.FirmwareDownloadState.Unknown, None),
        ("LATER", usb.FirmwareDownloadState.Unknown, None),
    ],
)
async def test_download_raw_status_text_is_preserved(
    text: str, state: usb.FirmwareDownloadState, code: int | None
) -> None:
    steps, raw = info(0xF0001, text)
    session, script = await opened(steps)
    reply = await session.firmware_download_status()
    assert (
        reply.state == state
        and reply.text == text
        and reply.failure_code == code
        and reply.raw == raw
    )
    assert script.remaining == 0


def test_research_commands_require_their_own_identifier_and_explicit_bounds() -> None:
    code = research.CommandCode(0xFFFFFFFF)
    assert (
        code.value == 0xFFFFFFFF
        and code != 0xFFFFFFFF
        and code != app80.CommandCode(0xFF)
    )
    with pytest.raises(TypeError):
        research.Request(3, bytes(16), reply_capacity=32)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        research.Request(app80.CommandCode(3), bytes(16), reply_capacity=32)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    for body, capacity in [(bytes(15), 32), (bytes(16), 15), (bytes(16), 0)]:
        with pytest.raises(ValueError):
            research.Request(code, body, reply_capacity=capacity)


@pytest.mark.parametrize("primary,secondary", [(0, 0), (1, 0xFFFF), (9, 3)])
async def test_research_fields_are_preserved_without_interpreting_dispositions(
    primary: int, secondary: int
) -> None:
    code = research.CommandCode(0x1A0001)
    body = bytes(16)
    request = research.Request(code, body, reply_capacity=32)
    wire = struct.pack("<4I", code.value, 0, 0, 0) + body
    raw = struct.pack("<4I", code.value, primary, secondary, 0) + bytes(range(16))
    session, script = await opened(command(wire, 32, raw))
    reply = await session.research_command(request)
    assert reply.code == code and reply.raw == raw and reply.body == bytes(range(16))
    assert (reply.primary, reply.secondary) == (primary, secondary)
    assert reply.status == (
        usb.CommandStatus.Success
        if primary == secondary == 0
        else usb.CommandStatus.DeviceError
    )
    assert script.remaining == 0 and session.state == usb.SessionState.Ready
    assert (request.code, request.body, request.reply_capacity) == (code, body, 32)


async def test_research_foreign_reply_prevents_further_requests() -> None:
    request = research.Request(
        research.CommandCode(0x1A0001), bytes(16), reply_capacity=32
    )
    wire = struct.pack("<8I", 0x1A0001, 0, 0, 0, 0, 0, 0, 0)
    raw = struct.pack("<8I", 0x1A0002, 1, 0xFFFF, 0, 0, 0, 0, 0)
    session, script = await opened(command(wire, 32, raw))
    with pytest.raises(usb.ProtocolError):
        (await session.research_command(request))
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        (await session.close())
    assert script.remaining == 0
