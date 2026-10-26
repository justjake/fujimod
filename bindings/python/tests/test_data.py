"""Composed data ranges use the production Rust core over synthetic USB."""

import pytest
from fujimod import usb
from test_stream import E, control, opened


def window(capacity: int = 4) -> usb.DataWindow:
    return usb.DataWindow(0x2000, 0x1000, 64, capacity, usb.OffsetMode.Wrapping)


async def test_wrapping_range_exclusively_owns_session_and_includes_tail() -> None:
    session, script = await opened(
        [
            E.control(control(0xE1, 4, offset=0xFFFFF000)),
            E.read(4, b"abcd"),
            E.control(control(0xE1, 2, offset=0xFFFFF004)),
            E.read(2, b"ef"),
        ],
        packet_bytes=1,
    )  # bulk command size is independent of debug packet size
    async with usb.data.read_chunks(session, window(), 0x1000, 6) as reader:
        assert session.state == usb.SessionState.ReadingRange
        with pytest.raises(usb.SessionBusyError):
            (await session.read_data(0, 1))
        assert [part async for part in reader] == [b"abcd", b"ef"]
        assert reader.bytes_completed == 6
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_failed_command_preserves_all_usb_fragments_and_prevents_cleanup() -> (
    None
):
    session, script = await opened([
        E.control(control(0xE1, 4, offset=0xFFFFF000)),
        E.read(4, b"abcd"),
        E.control(control(0xE1, 4, offset=0xFFFFF004)),
        E.read(4, b"e"),
        E.disconnected(3, b"f"),
    ])
    async with usb.data.read_chunks(session, window(), 0x1000, 8) as reader:
        assert (await anext(reader)) == b"abcd"
        with pytest.raises(usb.TransferError) as failure:
            (await anext(reader))
        assert reader.bytes_completed == 4
        assert failure.value.partial == b"ef"
        assert failure.value.transfer_error.partial == b"f"
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        (await session.empty_response())
    assert script.remaining == 0


async def test_invalid_mapping_or_range_leaves_session_ownership_unchanged() -> None:
    session, script = await opened()
    with pytest.raises(ValueError):
        usb.DataWindow(0x2000, 0x1000, 64, 4, usb.OffsetMode.Checked)
    with pytest.raises(TypeError):
        usb.DataWindow(0x2000, 0x1000, 64, 4, "Wrapping")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    for address, length in [(0xFFF, 1), (0x103F, 2), (0x1000, 0), (0xFFFFFFFF, 2)]:
        with pytest.raises(ValueError):
            usb.data.read_chunks(session, window(), address, length)
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_early_close_does_no_read_ahead_or_finalization() -> None:
    session, script = await opened([
        E.control(control(0xE1, 4, offset=0xFFFFF000)),
        E.read(4, b"abcd"),
    ])
    async with usb.data.read_chunks(session, window(), 0x1000, 8) as reader:
        assert (await anext(reader)) == b"abcd"
    assert [part async for part in reader] == []
    assert reader.bytes_completed == 4
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_empty_response_retains_raw_bytes_without_readiness_interpretation() -> (
    None
):
    session, script = await opened([
        E.control(control(0xE8, 32)),
        E.read(32, b"\x2a\x00\x00\x00"),
    ])
    assert (await session.empty_response()) == b"\x2a\x00\x00\x00"
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_control_disconnect_exposes_empty_partial_and_fences_session() -> None:
    session, script = await opened([
        E.control_disconnected(control(0xE1, 4, offset=0xFFFFF000))
    ])
    async with usb.data.read_chunks(session, window(), 0x1000, 4) as reader:
        with pytest.raises(usb.TransferError) as failure:
            (await anext(reader))
        assert failure.value.partial == b""
        assert failure.value.transferred == 0
    assert session.state == usb.SessionState.CompletionUnknown
    assert script.remaining == 0
