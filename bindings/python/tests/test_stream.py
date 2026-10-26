"""Exercise the production Rust session and Python ownership through replay."""

import gc
import struct
from collections.abc import Sequence
from inspect import isawaitable

import pytest
from fujimod import usb
from fujimod.usb import testing

E = testing.Exchange
TOKEN = 0x1234
LIMITS = usb.FirmwareLimits(
    memory_read=8, memory_write=8, configuration=256, console=256
)


def control(kind: int, size: int, *, token: int = TOKEN, offset: int = 0) -> bytes:
    return struct.pack(">BBIIH", kind, 0, offset, size, token)


async def opened(
    steps: Sequence[testing.Exchange] = (), *, packet_bytes: int = 4
) -> tuple[usb.Session, testing.Script]:
    return await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, struct.pack("<HH", TOKEN, 0) + struct.pack(">I", packet_bytes)),
            *steps,
        ],
        LIMITS,
    )


def memory_read(address: int, data: bytes) -> list[testing.Exchange]:
    request = struct.pack("<9I", 0x200001, 0, 0, 0, 2, 0, 0, address, len(data))
    return [
        E.control(control(0xD0, len(request))),
        E.write(request),
        E.control(control(0xD1, 36 + len(data))),
        E.read(36 + len(data), request + data),
    ]


async def test_stream_exclusively_owns_session_and_returns_exact_tail() -> None:
    session, script = await opened([
        *memory_read(0x1000, b"abcd"),
        *memory_read(0x1004, b"ef"),
        E.control(control(0xC4, 2)),
        E.read(2, TOKEN.to_bytes(2, "little")),
    ])
    async with usb.memory.read_chunks(session, 0x1000, 6) as reader:
        assert session.state == usb.SessionState.ReadingRange
        with pytest.raises(usb.SessionBusyError):
            (await session.ping())
        with pytest.raises(usb.SessionBusyError):
            (await session.close())
        assert (await anext(reader)) == b"abcd"
        assert reader.bytes_completed == 4
        assert (await anext(reader)) == b"ef"
        assert reader.bytes_completed == 6
        with pytest.raises(StopAsyncIteration):
            (await anext(reader))
    assert session.state == usb.SessionState.Ready
    assert (await session.close()) == TOKEN.to_bytes(2, "little")
    assert session.state == usb.SessionState.Closed
    assert script.remaining == 0


async def test_early_close_restores_session_without_issuing_extra_commands() -> None:
    session, script = await opened(memory_read(0x1000, b"abcd"))
    reader = usb.memory.read_chunks(session, 0x1000, 8)
    assert (await anext(reader)) == b"abcd"
    (await reader.aclose())
    assert session.state == usb.SessionState.Ready
    assert reader.bytes_completed == 4
    assert [part async for part in reader] == []
    assert script.remaining == 0


async def test_dropping_reader_returns_session_ownership_without_io() -> None:
    session, script = await opened()
    reader = usb.memory.read_chunks(session, 0x1000, 8)
    del reader
    gc.collect()
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_context_exception_returns_session_without_io() -> None:
    session, script = await opened()
    with pytest.raises(RuntimeError, match="consumer stopped"):
        async with usb.memory.read_chunks(session, 0x1000, 8):
            raise RuntimeError("consumer stopped")
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_stream_failure_keeps_delivered_bytes_and_fences_session() -> None:
    second = memory_read(0x1004, b"efgh")
    second[-1] = E.disconnected(40, b"\x01\x00")
    session, script = await opened([*memory_read(0x1000, b"abcd"), *second])
    sink = bytearray()
    reader = usb.memory.read_chunks(session, 0x1000, 8)
    sink.extend(await anext(reader))
    with pytest.raises(usb.TransferError) as failure:
        (await anext(reader))
    assert sink == b"abcd"
    assert reader.bytes_completed == 4
    assert failure.value.partial == b"\x01\x00"
    assert failure.value.transferred == 2
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        (await session.ping())
    (await reader.aclose())
    with pytest.raises(usb.CompletionUnknownError):
        usb.memory.read_chunks(session, 0x1000, 4)
    assert script.remaining == 0


async def test_invalid_range_and_oversize_direct_read_reject_before_io() -> None:
    session, script = await opened()
    with pytest.raises(ValueError):
        usb.memory.read_chunks(session, 0xFFFFFFFF, 2)
    with pytest.raises(ValueError):
        (
            await session.read_memory(0x1000, 6)
        )  # negotiated limit 4; direct command never splits
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_selector_and_mode_types_cannot_be_substituted() -> None:
    session, script = await opened()
    with pytest.raises(TypeError):
        (await session.device_info("Product"))  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        (await session.device_info(4))  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        (await session.prepare_data_write(4, usb.DataReadMode.Pattern))  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


async def test_camera_error_is_structured_and_does_not_wait_for_completion() -> None:
    steps = memory_read(0x1000, b"abcd")
    reply = bytearray(struct.pack("<5I", 0x200001, 1, 0xFFFF, 0, 2))
    steps[-1] = E.read(40, reply)
    session, script = await opened(steps)
    with pytest.raises(usb.DeviceCommandError) as failure:
        (await session.read_memory(0x1000, 4))
    assert (failure.value.command, failure.value.primary, failure.value.secondary) == (
        0x200001,
        1,
        0xFFFF,
    )
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


@pytest.mark.parametrize(
    "steps,error",
    [
        ([E.control_disconnected(control(0xC4, 2))], usb.TransferError),
        ([E.control(control(0xC4, 2)), E.disconnected(2, b"\x12")], usb.TransferError),
        ([E.control(control(0xC4, 2)), E.read(2, b"\x12")], usb.ProtocolError),
    ],
)
async def test_failed_close_is_unknown_and_cannot_be_retried_or_used(
    steps: list[testing.Exchange], error: type[Exception]
) -> None:
    session, script = await opened(steps)
    with pytest.raises(error):
        (await session.close())
    assert session.state == usb.SessionState.CompletionUnknown

    async def _async_149_22() -> bytes:
        return await session.read_memory(4096, 4)

    async def _async_152_22() -> usb.Reader:
        return usb.memory.read_chunks(session, 4096, 4)

    for operation in (session.close, session.ping, _async_149_22, _async_152_22):
        with pytest.raises(usb.CompletionUnknownError):
            result = operation()
            if isawaitable(result):
                await result
    assert session.state == usb.SessionState.CompletionUnknown
    assert script.remaining == 0


async def test_close_after_incomplete_command_cannot_claim_closed_or_send_c4() -> None:
    session, script = await opened([
        E.control(control(0xC2, 2)),
        E.disconnected(2, b""),
    ])
    with pytest.raises(usb.TransferError):
        (await session.ping())
    with pytest.raises(usb.CompletionUnknownError):
        (await session.close())
    assert session.state == usb.SessionState.CompletionUnknown
    assert script.remaining == 0


async def test_completed_close_preserves_opaque_reply_and_rejects_further_io() -> None:
    session, script = await opened([
        E.control(control(0xC4, 2)),
        E.read(2, b"\xfe\xff"),
    ])
    assert (await session.close()) == b"\xfe\xff"
    assert session.state == usb.SessionState.Closed

    async def _async_174_22() -> usb.Reader:
        return usb.memory.read_chunks(session, 4096, 4)

    for operation in (session.close, session.ping, _async_174_22):
        with pytest.raises(usb.ClosedSessionError):
            result = operation()
            if isawaitable(result):
                await result
    assert script.remaining == 0


@pytest.mark.parametrize("failed", [False, True])
async def test_abandon_releases_host_session_without_close_or_cleanup(
    failed: bool,
) -> None:
    steps = [E.control(control(0xC2, 2)), E.disconnected(2, b"")] if failed else []
    session, script = await opened(steps)
    if failed:
        with pytest.raises(usb.TransferError):
            (await session.ping())
    session.abandon()
    assert session.state == usb.SessionState.Abandoned

    async def _async_189_22() -> usb.Reader:
        return usb.memory.read_chunks(session, 4096, 4)

    for operation in (session.close, session.ping, session.abandon, _async_189_22):
        with pytest.raises(usb.ClosedSessionError):
            result = operation()
            if isawaitable(result):
                await result
    assert script.remaining == 0


async def test_abandon_cancels_an_idle_range_without_read_or_close_io() -> None:
    session, script = await opened()
    reader = usb.memory.read_chunks(session, 0x1000, 4)
    session.abandon()
    assert session.state == usb.SessionState.Abandoned
    await reader.aclose()
    with pytest.raises(usb.ClosedSessionError):
        await session.ping()
    assert script.remaining == 0
