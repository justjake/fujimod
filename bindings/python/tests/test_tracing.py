"""Trace the production Rust session over synthetic USB only."""

from collections.abc import Sequence

import pytest
from fujimod import usb
from fujimod.usb import testing, tracing
from test_stream import LIMITS, E, control, memory_read


async def opened(
    steps: Sequence[testing.Exchange] = (), *, trace: tracing.Trace
) -> tuple[usb.Session, testing.Script]:
    return await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, b"\x34\x12\x00\x00\x00\x00\x01\x00"),
            *steps,
        ],
        LIMITS,
        trace=trace,
    )


async def test_lifecycle_trace_is_typed_and_survives_session_close() -> None:
    trace = tracing.Trace(max_events=32, max_bytes=4096)
    session, script = await opened(
        [E.control(control(0xC4, 2)), E.read(2, b"\x34\x12")], trace=trace
    )
    (await session.close())
    batch = trace.drain()
    assert batch.dropped_events == batch.dropped_bytes == 0
    events = batch.events
    assert len(events) == 8
    assert sorted(e.sequence for e in [*events, *batch.protocol_events]) == list(
        range(12)
    )
    assert len(batch.protocol_events) == 4
    assert batch.protocol_events[0].layer == tracing.Layer.Ff80
    assert batch.protocol_events[0].command == 0xC0
    assert all(events[i].transfer == events[i + 1].transfer for i in range(0, 8, 2))
    assert events[0].stage == tracing.Stage.Control
    assert events[0].phase == tracing.Phase.Submitted
    assert events[0].request_kind == tracing.RequestKind.Open
    assert events[0].endpoint == 0
    assert events[0].requested == 12
    assert events[0].timestamp > 0
    assert events[-1].stage == tracing.Stage.BulkIn
    assert events[-1].transferred == 2
    assert events[-1].data == b"\x34\x12"
    with pytest.raises(AttributeError):
        events[0].sequence = 3  # ty: ignore[invalid-assignment]  # Verify runtime immutability.
    assert trace.drain().events == []
    assert script.remaining == 0


async def test_failed_transfer_keeps_partial_bytes_and_fencing() -> None:
    trace = tracing.Trace(32, 4096)
    session, script = await opened(
        [E.control(control(0xC2, 2)), E.disconnected(2, b"\x34")], trace=trace
    )
    with pytest.raises(usb.TransferError):
        (await session.ping())
    event = trace.drain().events[-1]
    assert event.phase == tracing.Phase.Failed
    assert event.failure_kind == tracing.FailureKind.Transfer
    assert event.data == b"\x34"
    assert event.transferred == 1
    assert session.state == usb.SessionState.CompletionUnknown
    assert script.remaining == 0


async def test_open_failure_can_be_drained_without_a_session() -> None:
    trace = tracing.Trace(32, 4096)
    with pytest.raises(usb.TransferError):
        (
            await testing.session(
                [E.control_disconnected(control(0xC0, 8, token=0))], LIMITS, trace=trace
            )
        )
    events = trace.drain().events
    assert len(events) == 2
    assert events[-1].phase == tracing.Phase.Failed
    assert events[0].data == control(0xC0, 8, token=0)


async def test_trace_overflow_reports_loss_without_changing_camera_exchange() -> None:
    trace = tracing.Trace(max_events=2, max_bytes=4)
    session, script = await opened(trace=trace)
    batch = trace.drain()
    assert batch.dropped_events == 4
    assert (
        batch.dropped_bytes == 40
    )  # USB and FF80 retain complete requests and replies
    assert len(batch.events) == 2
    assert all(e.data == b"" for e in batch.events)
    assert session.state == usb.SessionState.Ready
    assert trace.drain().dropped_events == 0
    assert script.remaining == 0


async def test_trace_can_be_drained_while_range_reader_owns_session() -> None:
    trace = tracing.Trace(32, 4096)
    session, script = await opened(memory_read(0x1000, b"abcd"), trace=trace)
    async with usb.memory.read_chunks(session, 0x1000, 4) as reader:
        assert session.state == usb.SessionState.ReadingRange
        assert len(trace.drain().events) == 4
        assert (await anext(reader)) == b"abcd"
        events = trace.drain().events
        assert events[2].stage == tracing.Stage.BulkOut
        assert events[2].data[:4] == b"\x01\x00\x20\x00"
        assert events[-1].data.endswith(b"abcd")
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0


def test_invalid_trace_bounds_reject_before_allocating_or_opening() -> None:
    for events, bytes_ in [(0, 1), (1, 0)]:
        with pytest.raises(ValueError):
            tracing.Trace(events, bytes_)


async def test_completed_usb_transfer_does_not_certify_a_valid_protocol_reply() -> None:
    trace = tracing.Trace(32, 4096)
    session, script = await opened(
        [E.control(control(0xC2, 2)), E.read(2, b"\x34")], trace=trace
    )
    with pytest.raises(usb.ProtocolError):
        (await session.ping())
    batch = trace.drain()
    event = batch.events[-1]
    assert event.phase == tracing.Phase.Completed
    assert event.data == b"\x34"
    assert batch.protocol_events[-1].phase == tracing.Phase.Failed
    assert batch.protocol_events[-1].data == b"\x34"
    assert session.state == usb.SessionState.CompletionUnknown
    assert script.remaining == 0
