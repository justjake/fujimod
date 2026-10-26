"""Checked sequences use production framing and stop before dependent writes."""

import struct

import pytest
from fujimod import usb
from test_async import opened, run
from test_stream import E, control, memory_read


def write(address: int, data: bytes) -> list[E]:
    request = struct.pack("<9I", 0x200001, 0, 0, 0, 3, 0, 0, address, len(data))
    return [
        E.control(control(0xD0, len(request) + len(data))),
        E.write(request + data),
        E.control(control(0xD1, 36)),
        E.read(36, request),
    ]


def test_sequence_reads_writes_guards_and_explicit_query_status() -> None:
    async def test() -> None:
        request = struct.pack("<8I", 0x450006, 0, 0, 0, 77, 0, 0, 0)
        reply = struct.pack("<8I", 0x450006, 1, 0xFFFF, 0, 0, 3, 0, 0)
        session, script = await opened([
            *memory_read(0x1000, b"old!"),
            *write(0x1000, b"new!"),
            *memory_read(0x1000, b"new!"),
            E.control(control(0xD0, 32)),
            E.write(request),
            E.control(control(0xD1, 32)),
            E.read(32, reply),
        ])
        plan = usb.CommandSequence()
        before = plan.read(0x1000, 4, b"old!")
        sent = plan.write(0x1000, b"new!")
        after = plan.read(0x1000, 4, b"new!")
        queried = plan.query(77, [(0, 0), (1, 0xFFFF)])
        replies = await session.execute_sequence(plan)
        assert [replies[i] for i in (before, sent, after, queried)] == [
            b"old!",
            b"",
            b"new!",
            reply,
        ]
        assert script.remaining == 0 and session.state == usb.SessionState.Ready

    run(test)


@pytest.mark.parametrize(
    "fault", ["contents", "short", "long", "status", "reserved", "echo"]
)
def test_failed_guard_fences_session_before_later_write(fault: str) -> None:
    async def test() -> None:
        steps = memory_read(0x1000, b"old!")
        response = struct.pack("<9I", 0x200001, 0, 0, 0, 2, 0, 0, 0x1000, 4) + b"old!"
        if fault == "short":
            response = response[:-1]
        if fault == "long":
            response += b"x"
        if fault == "status":
            response = response[:4] + struct.pack("<I", 7) + response[8:]
        if fault == "reserved":
            response = response[:12] + struct.pack("<I", 7) + response[16:]
        if fault == "echo":
            response = bytes(4) + response[4:]
        steps[-1] = E.read(40, response)
        session, script = await opened([*steps, *write(0x2000, b"BAD!")])
        plan = usb.CommandSequence()
        plan.read(0x1000, 4, b"else" if fault == "contents" else b"old!")
        plan.write(0x2000, b"BAD!")
        with pytest.raises(usb.ProtocolError):
            await session.execute_sequence(plan)
        assert (
            script.remaining == 4
            and session.state == usb.SessionState.CompletionUnknown
        )
        with pytest.raises(usb.CompletionUnknownError):
            await session.ping()

    run(test)


def test_entire_sequence_is_validated_before_first_io() -> None:
    async def test() -> None:
        session, script = await opened(memory_read(0x1000, b"old!"))
        plan = usb.CommandSequence()
        plan.read(0x1000, 4)
        plan.write(0x2000, b"12345")  # the session advertises four-byte packets
        with pytest.raises(ValueError):
            await session.execute_sequence(plan)
        assert script.remaining == 4 and session.state == usb.SessionState.Ready

    run(test)
