"""App80 request types and mailbox exchange through the real Rust core."""

import struct

import pytest
from fujimod import usb
from fujimod.usb import app80, testing
from test_stream import LIMITS, E, control, opened


def config_write(
    offset: int, data: bytes | bytearray, *, page: int = 0
) -> list[testing.Exchange]:
    command = struct.pack("<9I", 0x1A0002, 0, 0, 0, 3, 1, page, offset, len(data))
    return [
        E.control(control(0xD0, 36 + len(data))),
        E.write(command + data),
        E.control(control(0xD1, 36)),
        E.read(36, command),
    ]


def config_read(
    offset: int, data: bytes | bytearray, *, page: int = 0
) -> list[testing.Exchange]:
    command = struct.pack("<9I", 0x1A0002, 0, 0, 0, 2, 1, page, offset, len(data))
    return [
        E.control(control(0xD0, 36)),
        E.write(command),
        E.control(control(0xD1, 36 + len(data))),
        E.read(36 + len(data), command + data),
    ]


async def test_exchange_uses_selected_mailbox_and_keeps_result_opaque() -> None:
    request = app80.Request(app80.CommandCode(0x22), b"\xff")
    assert request.raw == b"\x22\xff" + bytes(31)
    result = b"\x01" + bytes(range(32))
    session, script = await opened([
        *config_write(0x181, request.raw[1:], page=2),
        *config_write(0x180, request.raw, page=2),
        *config_read(0x180, result, page=2),
    ])
    mailbox = app80.Mailbox(page=2, offset=0x180, configuration_limit=256)
    assert (await usb.app80.exchange(session, mailbox, request)) == result
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


def test_types_reject_wrong_selector_and_oversize_or_incomplete_definition() -> None:
    with pytest.raises(TypeError):
        app80.Request(1, b"")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        app80.Request(usb.InfoSelector.Vendor, b"")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(ValueError):
        app80.Request(app80.CommandCode(1), bytes(33))
    with pytest.raises(OverflowError):
        app80.CommandCode(256)
    with pytest.raises(ValueError):
        app80.Mailbox(page=0, offset=0x80, configuration_limit=32)
    with pytest.raises(ValueError):
        app80.Mailbox(page=0, offset=0xFFFFFFE0, configuration_limit=256)
    assert app80.Request(app80.CommandCode(0), b"").raw == bytes(33)


async def test_publish_failure_never_reads_mailbox_or_retries() -> None:
    request = app80.Request(app80.CommandCode(1), b"")
    session, script = await opened([
        *config_write(0x81, request.raw[1:]),
        E.control_disconnected(control(0xD0, 69)),
    ])
    with pytest.raises(usb.TransferError):
        (
            await usb.app80.exchange(
                session,
                app80.Mailbox(page=0, offset=0x80, configuration_limit=256),
                request,
            )
        )
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        (await session.ping())
    assert script.remaining == 0


async def test_device_error_one_is_returned_without_polling() -> None:
    request = app80.Request(app80.CommandCode(2), b"")
    publish = config_write(0x80, request.raw)
    error = struct.pack("<9I", 0x1A0002, 1, 0xFFFF, 0, 3, 1, 0, 0x80, 33)
    publish[-1] = E.read(36, error)
    session, script = await opened([*config_write(0x81, request.raw[1:]), *publish])
    with pytest.raises(usb.DeviceCommandError) as failure:
        (
            await usb.app80.exchange(
                session,
                app80.Mailbox(page=0, offset=0x80, configuration_limit=256),
                request,
            )
        )
    assert (failure.value.primary, failure.value.secondary) == (1, 0xFFFF)
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_whole_exchange_budget_is_checked_before_first_write() -> None:
    session, script = await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, b"\x34\x12\x00\x00\x00\x00\x01\x00"),
        ],
        LIMITS,
        max_reply_bytes=68,
    )
    with pytest.raises(ValueError):
        (
            await usb.app80.exchange(
                session,
                app80.Mailbox(page=0, offset=0x80, configuration_limit=256),
                app80.Request(app80.CommandCode(1), b""),
            )
        )
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_session_firmware_limit_cannot_be_bypassed_by_a_mailbox_definition() -> (
    None
):
    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=32, console=256
    )
    session, script = await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, b"\x34\x12\x00\x00\x00\x00\x01\x00"),
        ],
        limits,
    )
    with pytest.raises(ValueError):
        (
            await usb.app80.exchange(
                session,
                app80.Mailbox(page=0, offset=0x80, configuration_limit=256),
                app80.Request(app80.CommandCode(1), b""),
            )
        )
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


def test_command_codes_are_immutable_values_distinct_from_other_selectors() -> None:
    code = app80.CommandCode(2)
    assert code == app80.CommandCode(2)
    assert {code: "selected"}[app80.CommandCode(2)] == "selected"
    assert code != 2
    with pytest.raises(AttributeError):
        code.value = 3  # ty: ignore[invalid-assignment]  # Verify runtime immutability.
