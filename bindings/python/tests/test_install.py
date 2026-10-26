"""Mod helpers execute native framing with guarded ownership and no discovery."""

import asyncio
import struct

import pytest
from fujimod import usb
from test_async import opened, run
from test_sequence import write
from test_stream import LIMITS, E, control, memory_read


def query(tag: int, primary: int = 0, secondary: int = 0) -> list[E]:
    request = struct.pack("<8I", 0x450006, 0, 0, 0, tag, 0, 0, 0)
    reply = struct.pack("<8I", 0x450006, primary, secondary, 0, 0, 0, 0, 0)
    return [
        E.control(control(0xD0, 32)),
        E.write(request),
        E.control(control(0xD1, 32)),
        E.read(32, reply),
    ]


def test_borrowed_callback_indices_and_restoration() -> None:
    async def test() -> None:
        stock, target = struct.pack("<I", 0x9000), struct.pack("<I", 0x8000)
        session, script = await opened([
            *memory_read(0x1000, stock),
            *memory_read(0x1100, b"orig"),
            *write(0x1100, b"seed"),
            *memory_read(0x1100, b"seed"),
            *write(0x1000, target),
            *memory_read(0x1000, target),
            *query(77),
            *memory_read(0x1000, target),
            *memory_read(0x1100, b"done"),
            *write(0x1000, stock),
            *memory_read(0x1000, stock),
            *write(0x1100, b"orig"),
            *memory_read(0x1100, b"orig"),
        ])
        plan = usb.InstallQueries(0x1000, 0x9000, 0x1100, b"orig")
        queried, result = plan.call(0x8000, 77, [(0, 0)], b"seed")
        replies = await session.execute_sequence(plan.finish())
        assert replies[result] == b"done" and len(replies[queried]) == 32
        assert session.state == usb.SessionState.Ready and script.remaining == 0

    run(test)


def test_verified_write_batches_use_firmware_and_session_limits() -> None:
    async def test() -> None:
        session, script = await opened([
            *write(0x4000, b"abcd"),
            *memory_read(0x4000, b"abcd"),
            *write(0x4004, b"efgh"),
            *memory_read(0x4004, b"efgh"),
            *write(0x4008, b"i"),
            *memory_read(0x4008, b"i"),
        ])
        batches = usb.CommandSequence.verified_writes(
            0x4000, b"abcdefghi", LIMITS, session.packet_bytes
        )
        assert len(batches) == 1
        for batch in batches:
            await session.execute_sequence(batch)
        assert script.remaining == 0

    run(test)


def upload_steps() -> list[E]:
    setup = struct.pack("<8I", 0x760001, 0, 0, 0, 0, 16, 0, 0)
    return [
        E.control(control(0xD0, 32)),
        E.write(setup),
        E.control(control(0xD1, 32)),
        E.read(32, setup),
        E.control(control(0xE0, 8)),
        E.write(b"hello wo"),
        E.control(control(0xE0, 8)),
        E.write(b"rld" + bytes(5)),
        E.control(control(0xE8, 32)),
        E.read(32, setup),
    ]


def pointer_steps() -> tuple[list[E], list[E]]:
    old, new = struct.pack("<I", 0x8000), struct.pack("<I", 0x4000)
    before = [
        *memory_read(0x1000, old),
        *memory_read(0x1004, struct.pack("<I", 32)),
        *write(0x1000, new),
        *memory_read(0x1000, new),
    ]
    after = [*memory_read(0x1000, new), *write(0x1000, old), *memory_read(0x1000, old)]
    return before, after


def test_owned_upload_padding_restore_and_full_bulk_readback() -> None:
    async def test() -> None:
        before, after = pointer_steps()
        steps = before + upload_steps() + after
        for offset, data in ((0, b"hell"), (4, b"o wo"), (8, b"rld")):
            steps += [
                E.control(control(0xE1, len(data), offset=offset)),
                E.read(len(data), data),
            ]
        session, script = await opened(steps)
        image = usb.OwnedUpload(0x4000, 16, 0x4000, b"hello world", 8, 4)
        assert image.padded_bytes == 16 and image.image_bytes == 11
        result = await session.upload_owned(image, 0x1000, 0x8000, 0x1004, 32)
        assert result.bytes_written == 16
        await session.verify_owned(
            image, usb.DataWindow(0x4000, 0x4000, 16, 4, usb.OffsetMode.Checked)
        )
        assert script.remaining == 0 and session.state == usb.SessionState.Ready

    run(test)


def test_canceled_upload_retains_unknown_destination_without_restoration() -> None:
    async def test() -> None:
        before, after = pointer_steps()
        steps = upload_steps()
        steps[-1] = E.pending_read(32)
        session, script = await opened(before + steps + after)
        image = usb.OwnedUpload(0x4000, 16, 0x4000, b"hello world", 8, 4)
        task = asyncio.create_task(
            session.upload_owned(image, 0x1000, 0x8000, 0x1004, 32)
        )

        async def dispatched() -> None:
            while script.remaining != len(after):
                await asyncio.sleep(0)

        await asyncio.wait_for(dispatched(), 2)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert (
            script.remaining == len(after)
            and session.state == usb.SessionState.CompletionUnknown
        )

    run(test)


def test_invalid_padding_is_rejected_when_constructing_the_image() -> None:
    with pytest.raises(ValueError):
        usb.OwnedUpload(0x4000, 11, 0x4000, b"hello world", 8, 4)
