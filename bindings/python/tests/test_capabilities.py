"""Capabilities metadata is one request; reading its table is a separate action."""

import struct

import pytest
from fujimod import usb
from fujimod.usb import testing

LIMITS = usb.FirmwareLimits(memory_read=1, memory_write=1, configuration=1, console=1)


async def session(reply: bytes) -> tuple[usb.Session, testing.Script]:
    exchange = testing.Exchange

    def control(kind: int, size: int, token: int) -> testing.Exchange:
        return exchange.control(struct.pack(">BBIIH", kind, 0, 0, size, token))

    request = struct.pack("<9I", 0x580000, 0, 0, 0, 0, 0x100, 0, 0, 0)
    return await testing.session(
        [
            control(0xC0, 8, 0),
            exchange.read(8, b"\x34\x12\0\0\0\0\x01\0"),
            control(0xD0, 36, 0x1234),
            exchange.write(request),
            control(0xD1, 32, 0x1234),
            exchange.read(32, reply),
        ],
        LIMITS,
    )


@pytest.mark.parametrize("length", [0, 48, 128, 0xFFFFFFFF])
async def test_metadata_retains_firmware_size_without_implicit_table_read(
    length: int,
) -> None:
    raw = struct.pack("<8I", 0x580000, 0, 0, 0, 0, length, 0x12345678, 0x87654321)
    camera, script = await session(raw)
    reply = await camera.prepare_capabilities()
    assert reply.byte_length == length and reply.raw == raw
    assert script.remaining == 0 and camera.state == usb.SessionState.Ready
    with pytest.raises(AttributeError):
        reply.byte_length = 1  # ty: ignore[invalid-assignment]  # Deliberately invalid input: verify runtime rejection.


@pytest.mark.parametrize(
    "function,exception,state",
    [
        (0, usb.DeviceCommandError, usb.SessionState.Ready),
        (1, usb.ProtocolError, usb.SessionState.CompletionUnknown),
    ],
)
async def test_error_reply_is_complete_only_when_its_header_matches(
    function: int, exception: type[Exception], state: usb.SessionState
) -> None:
    camera, script = await session(
        struct.pack("<8I", 0x580000, 1, 0xFFFF, 0, function, 48, 0, 0)
    )
    with pytest.raises(exception) as error:
        (await camera.prepare_capabilities())
    assert script.remaining == 0 and camera.state == state
    if state == usb.SessionState.Ready:
        assert isinstance(error.value, usb.DeviceCommandError)
        assert (error.value.command, error.value.primary, error.value.secondary) == (
            0x580000,
            1,
            0xFFFF,
        )
    else:
        with pytest.raises(usb.CompletionUnknownError):
            (await camera.close())
