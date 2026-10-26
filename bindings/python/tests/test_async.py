"""Await the production Rust core; no discovery or physical USB is permitted."""

import asyncio
import gc
import importlib
import struct
from collections.abc import Callable, Coroutine, Sequence
from unittest.mock import patch

import pytest
from fujimod import usb
from fujimod.usb import app80, testing, tracing
from test_app80 import config_read, config_write
from test_service_commands import info
from test_stream import LIMITS, TOKEN, E, control, memory_read


async def opened(
    steps: Sequence[testing.Exchange] = (),
    *,
    trace: tracing.Trace | None = None,
    limits: usb.FirmwareLimits = LIMITS,
) -> tuple[usb.Session, testing.Script]:
    return await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, struct.pack("<HH", TOKEN, 0) + struct.pack(">I", 4)),
            *steps,
        ],
        limits,
        trace=trace,
    )


async def dispatched(script: testing.Script) -> None:
    # Await observed dispatch, never elapsed-time guesses. A stalled task fails
    # with a bounded timeout instead of hanging the test process.
    async def wait() -> None:
        while script.remaining:
            await asyncio.sleep(0)

    await asyncio.wait_for(wait(), 2)


def run(test: Callable[[], Coroutine[object, object, None]]) -> None:
    with (
        patch.object(
            usb, "discover", side_effect=AssertionError("Physical USB forbidden")
        ),
        patch.object(
            usb, "discover", side_effect=AssertionError("Physical USB forbidden")
        ),
    ):
        return asyncio.run(test())


def test_async_namespaces_are_directly_importable() -> None:
    assert importlib.import_module("fujimod.usb.memory") is usb.memory
    assert importlib.import_module("fujimod.usb.data") is usb.data
    assert usb.Session.__module__ == "fujimod.usb"
    assert not hasattr(usb.Session, "read_range")


def test_native_coroutines_work_with_create_task_and_multiple_event_loops() -> None:
    async def test() -> None:
        s, script = await opened([
            E.control(control(0xC2, 2)),
            E.read(2, b"xy"),
            E.control(control(0xC4, 2)),
            E.read(2, b"ab"),
        ])
        assert (s.token, s.packet_bytes) == (TOKEN, 4)
        assert await asyncio.create_task(s.ping()) == b"xy"
        assert await s.close() == b"ab"
        assert s.state == usb.SessionState.Closed and script.remaining == 0
        with pytest.raises(usb.ClosedSessionError):
            await s.ping()

    run(test)
    run(test)


def test_canceling_actual_transfer_restores_fenced_owner_and_trace() -> None:
    async def test() -> None:
        trace = tracing.Trace(64, 4096)
        s, script = await opened(
            [E.control(control(0xC2, 2)), E.pending_read(2)], trace=trace
        )
        task = asyncio.create_task(s.ping())
        await dispatched(script)
        assert s.state == usb.SessionState.InFlight
        with pytest.raises(usb.SessionBusyError):
            await s.close()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert s.state == usb.SessionState.CompletionUnknown
        with pytest.raises(usb.CompletionUnknownError):
            await s.ping()
        with pytest.raises(usb.CompletionUnknownError):
            await s.close()
        assert any(e.phase == tracing.Phase.Canceled for e in trace.drain().events)
        assert script.remaining == 0

    run(test)


def test_cancellation_before_dispatch_does_not_consume_or_fence_session() -> None:
    async def test() -> None:
        s, script = await opened()
        task = asyncio.create_task(s.ping())
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


@pytest.mark.parametrize("operation", ["ping", "close"])
def test_abandon_cancels_inflight_without_cleanup(operation: str) -> None:
    async def test() -> None:
        s, script = await opened([
            E.control(control(0xC2 if operation == "ping" else 0xC4, 2)),
            E.pending_read(2),
        ])
        task = asyncio.create_task(getattr(s, operation)())
        await dispatched(script)
        s.abandon()
        assert s.state == usb.SessionState.Abandoned
        with pytest.raises(usb.ClosedSessionError):
            await task
        assert s.state == usb.SessionState.Abandoned and script.remaining == 0

    run(test)


@pytest.mark.parametrize("failure", ["cancel", "short", "disconnect"])
def test_failed_close_is_consumed(failure: str) -> None:
    async def test() -> None:
        reply = {
            "cancel": E.pending_read(2),
            "short": E.read(2, b"x"),
            "disconnect": E.disconnected(2, b""),
        }[failure]
        s, script = await opened([E.control(control(0xC4, 2)), reply])
        task = asyncio.create_task(s.close())
        if failure == "cancel":
            await dispatched(script)
            task.cancel()
        with pytest.raises(
            asyncio.CancelledError if failure == "cancel" else usb.UsbError
        ):
            await task
        assert s.state == usb.SessionState.CompletionUnknown
        with pytest.raises(usb.CompletionUnknownError):
            await s.close()
        assert script.remaining == 0

    run(test)


def test_chunk_iterator_holds_session_and_preserves_exact_tail() -> None:
    async def test() -> None:
        s, script = await opened([
            *memory_read(0x1000, b"abcd"),
            *memory_read(0x1004, b"ef"),
        ])
        reader = usb.memory.read_chunks(s, 0x1000, 6)
        async with reader as entered:
            assert entered is reader
            assert s.state == usb.SessionState.ReadingRange
            with pytest.raises(usb.SessionBusyError):
                await s.ping()
            assert [b async for b in reader] == [b"abcd", b"ef"]
        assert reader.bytes_completed == 6
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


@pytest.mark.parametrize("exit_kind", ["early", "exception", "drop"])
def test_reader_early_exit_releases_owner_without_io(exit_kind: str) -> None:
    async def test() -> None:
        s, script = await opened()
        reader = usb.memory.read_chunks(s, 0x1000, 8)
        if exit_kind == "early":
            await reader.aclose()
            assert [b async for b in reader] == []
        elif exit_kind == "exception":
            with pytest.raises(RuntimeError, match="consumer stopped"):
                async with reader:
                    raise RuntimeError("consumer stopped")
        else:
            del reader
            gc.collect()
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


@pytest.mark.parametrize("exit_kind", ["cancel", "aclose", "abandon"])
def test_pending_chunk_cancellation_closes_iterator_and_fences_owner(
    exit_kind: str,
) -> None:
    async def test() -> None:
        steps = memory_read(0x1000, b"abcd")
        steps[-1] = E.pending_read(40)
        s, script = await opened(steps)
        reader = usb.memory.read_chunks(s, 0x1000, 8)
        task = asyncio.create_task(anext(reader))
        await dispatched(script)
        with pytest.raises(usb.SessionBusyError):
            await anext(reader)
        if exit_kind == "cancel":
            task.cancel()
        elif exit_kind == "aclose":
            await asyncio.wait_for(asyncio.gather(reader.aclose(), reader.aclose()), 2)
        else:
            s.abandon()
        with pytest.raises(
            asyncio.CancelledError if exit_kind == "cancel" else usb.ClosedSessionError
        ):
            await task
        await reader.aclose()
        assert s.state == (
            usb.SessionState.Abandoned
            if exit_kind == "abandon"
            else usb.SessionState.CompletionUnknown
        )
        assert reader.bytes_completed == 0 and script.remaining == 0
        assert [b async for b in reader] == []

    run(test)


def test_composed_read_write_and_data_window_use_same_core() -> None:
    async def test() -> None:
        write = struct.pack("<9I", 0x200001, 0, 0, 0, 3, 0, 0, 0x2000, 2)
        steps = [
            *memory_read(0x1000, b"abcd"),
            *memory_read(0x1004, b"ef"),
            E.control(control(0xD0, 38)),
            E.write(write + b"hi"),
            E.control(control(0xD1, 36)),
            E.read(36, write),
            E.control(control(0xE1, 3, offset=5)),
            E.read(3, b"xyz"),
        ]
        s, script = await opened(steps)
        assert await usb.memory.read_range(s, 0x1000, 6) == b"abcdef"
        await usb.memory.write_range(s, 0x2000, b"hi")
        window = usb.DataWindow(0x1000, 0x1000, 32, 4, usb.OffsetMode.Checked)
        async with usb.data.read_chunks(s, window, 0x1005, 3) as reader:
            assert [b async for b in reader] == [b"xyz"]
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


def test_app80_composition_preserves_firmware_result_without_waiting() -> None:
    async def test() -> None:
        request = app80.Request(app80.CommandCode(0x22), b"hi")
        reply = b"\x01" + bytes(range(32))
        s, script = await opened([
            *config_write(0x181, request.raw[1:], page=2),
            *config_write(0x180, request.raw, page=2),
            *config_read(0x180, reply, page=2),
        ])
        mailbox = app80.Mailbox(page=2, offset=0x180, configuration_limit=256)
        assert await app80.exchange(s, mailbox, request) == reply
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


def test_application_writing_is_data_and_wrong_enums_fail_before_io() -> None:
    async def test() -> None:
        steps, raw = info(0xF0001, "WRITING")
        s, script = await opened(steps)
        for bad in ["Product", 4, usb.CameraMode.Record]:
            with pytest.raises(TypeError):
                await s.device_info(bad)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        with pytest.raises(ValueError):
            await s.read_memory(0x1000, 5)
        with pytest.raises(TypeError):
            await s.prepare_data_write(4, usb.DataReadMode.Echo)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        reply = await s.firmware_download_status()
        assert reply.state == usb.FirmwareDownloadState.Writing and reply.raw == raw
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


def test_native_wakeup_resumes_asyncio_without_busy_polling_or_worker_runtime() -> None:
    async def test() -> None:
        s, script = await opened([
            E.control(control(0xC2, 2)),
            E.delayed_read(2, b"OK", 20),
        ])
        task = asyncio.create_task(s.ping())
        await dispatched(script)
        assert await asyncio.wait_for(task, 2) == b"OK"
        assert s.state == usb.SessionState.Ready

    run(test)


def test_diagnostic_and_upload_dtos_are_shared_across_async_and_blocking() -> None:
    from test_service_commands import command

    async def test() -> None:
        query = struct.pack("<8I", 0x450006, 0, 0, 0, 0x80, 0, 0, 0)
        query_reply = struct.pack("<8I", 0x450006, 1, 0xFFFF, 0, 9, 8, 7, 6)
        setup = struct.pack("<8I", 0x760001, 0, 0, 0, 0, 3, 1, 0)
        setup_reply = struct.pack("<8I", 0x760001, 0, 0, 0, 9, 8, 7, 6)
        s, script = await opened([
            *command(query, 32, query_reply),
            *command(setup, 32, setup_reply),
            E.control(control(0xE0, 2)),
            E.write(b"ab"),
            E.control(control(0xE0, 1)),
            E.write(b"c"),
            E.control(control(0xE8, 32)),
            E.read(32, b"\xff\x01\x80\x7f"),
        ])
        reply = await s.diagnostic_query(usb.DiagnosticTag.bulk_source())
        assert reply.status == usb.CommandStatus.DeviceError
        assert (reply.primary, reply.secondary) == (
            1,
            0xFFFF,
        ) and reply.raw == query_reply
        result = await usb.data.upload(
            s, b"abc", piece_bytes=2, mode=usb.DataWriteMode.FillOnes
        )
        assert isinstance(result, usb.data.UploadReply) and isinstance(
            result, usb.data.UploadReply
        )
        assert result.setup.status == usb.CommandStatus.Success
        assert result.setup.fields == [9, 8, 7, 6]
        assert (result.bytes_written, result.acknowledgment) == (3, b"\xff\x01\x80\x7f")
        assert s.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


def test_data_disconnect_retains_prior_chunks_and_partial_completion() -> None:
    async def test() -> None:
        s, script = await opened([
            E.control(control(0xE1, 4)),
            E.read(4, b"abcd"),
            E.control(control(0xE1, 4, offset=4)),
            E.read(4, b"e"),
            E.disconnected(3, b"f"),
        ])
        window = usb.DataWindow(0x1000, 0x1000, 8, 4, usb.OffsetMode.Checked)
        async with usb.data.read_chunks(s, window, 0x1000, 8) as reader:
            assert await anext(reader) == b"abcd"
            with pytest.raises(usb.TransferError) as failure:
                await anext(reader)
            assert failure.value.partial == b"ef"
            assert failure.value.transfer_error.partial == b"f"
            assert reader.bytes_completed == 4
        assert s.state == usb.SessionState.CompletionUnknown and script.remaining == 0

    run(test)
