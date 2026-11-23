"""Owned bulk-memory loans over the production Rust session.

The camera is a synthetic publication-source profile: a borrowed eight-byte
source pair in a small RAM window, a stock-code digest, one probe and one scope.
Every expected exchange below is built from that profile, never from a real
body: the session, mapping and Python ownership are the production code, and
the transport is `fujimod.usb.testing` replay.
"""

from __future__ import annotations

import asyncio
import gc
import hashlib
import json
import struct
from collections.abc import Sequence
from pathlib import Path

import pytest
from fujimod import _native, usb
from fujimod.remote import Camera
from fujimod.usb import testing

E = testing.Exchange
TOKEN = 0x1234
LIMITS = usb.FirmwareLimits(
    memory_read=64, memory_write=64, configuration=256, console=256
)
PACKET_BYTES = 65536

SCHEMA_URI = "https://fujimod.com/schemas/bulk.generated.schema.json"
WINDOW = (0x1000, 0x1000)
SOURCE_STATE = 0x1200
CAPACITY = 32
DATA = (0x1800, 0x40)
CODE = b"stock code"
ORIGINAL = struct.pack("<2I", 0xDEAD0000, 16)
SELECTED = struct.pack("<2I", 0, CAPACITY)


def profile_json(**overrides: object) -> str:
    """One tiny bounded publication profile; nothing here is a camera fact."""
    facts: dict[str, object] = {
        "$schema": SCHEMA_URI,
        "schema": 1,
        "id": "sample-1-bulk",
        "firmware_id": "sample-1",
        "product": "SYNTHETIC",
        "internal_version": "0.0",
        "code_alias": 0,
        "source": {
            "mechanism": "publication",
            "source_state": SOURCE_STATE,
            "capacity": CAPACITY,
        },
        "window": {"address": WINDOW[0], "size": WINDOW[1]},
        "code_checks": [
            {
                "name": "entry",
                "address": WINDOW[0],
                "size": len(CODE),
                "sha256": hashlib.sha256(CODE).hexdigest(),
            }
        ],
        "probes": [{"address": WINDOW[0], "size": 4}],
        "scopes": [
            {
                "name": "quick",
                "basis": "Synthetic test range.",
                "ranges": [
                    {"name": "data", "address": DATA[0], "size": DATA[1]},
                ],
            }
        ],
    }
    facts.update(overrides)
    return json.dumps(facts)


PROFILE = usb.BulkProfile.from_json(profile_json())


def control(kind: int, size: int, *, offset: int = 0, token: int = TOKEN) -> bytes:
    return struct.pack(">BBIIH", kind, 0, offset, size, token)


def exchange_max(request: bytes, reply: bytes, maximum: int) -> list[testing.Exchange]:
    return [
        E.control(control(0xD0, len(request))),
        E.write(request),
        E.control(control(0xD1, maximum)),
        E.read(maximum, reply),
    ]


def exchange(request: bytes, reply: bytes) -> list[testing.Exchange]:
    return exchange_max(request, reply, len(reply))


def memory_request(function: int, address: int, size: int) -> bytes:
    return struct.pack("<9I", 0x200001, 0, 0, 0, function, 0, 0, address, size)


def read(address: int, data: bytes) -> list[testing.Exchange]:
    return exchange(
        memory_request(2, address, len(data)),
        memory_request(2, address, len(data)) + data,
    )


def write(address: int, data: bytes) -> list[testing.Exchange]:
    request = memory_request(3, address, len(data)) + data
    return exchange(request, memory_request(3, address, len(data)))


def data_read(offset: int, data: bytes) -> list[testing.Exchange]:
    return [
        E.control(control(0xE1, len(data), offset=offset)),
        E.read(len(data), data),
    ]


def info(selector: int, text: str) -> list[testing.Exchange]:
    request = bytearray(32)
    struct.pack_into("<I", request, 0, 0x150000)
    struct.pack_into("<I", request, 16, selector)
    reply = bytes(request[:16]) + text.encode("utf-16-le") + b"\0\0"
    return exchange_max(bytes(request), reply, 128)


def pending_memory_read(address: int, size: int) -> list[testing.Exchange]:
    """One memory read whose reply never arrives: the command is in flight."""
    return [
        E.control(control(0xD0, 36)),
        E.write(memory_request(2, address, size)),
        E.control(control(0xD1, 36 + size)),
        E.pending_read(36 + size),
    ]


CANARY = E.control(b"\xaa" * 12)


async def opened(steps: Sequence[testing.Exchange] = ()) -> tuple[usb.Session, testing.Script]:
    return await testing.session(
        [
            E.control(control(0xC0, 8, token=0)),
            E.read(8, struct.pack("<HH", TOKEN, 0) + struct.pack(">I", PACKET_BYTES)),
            *steps,
        ],
        LIMITS,
    )


async def dispatched_until(script: testing.Script, remaining: int) -> None:
    """Await observed dispatch, never an elapsed-time guess."""

    async def wait() -> None:
        while script.remaining > remaining:
            await asyncio.sleep(0)

    await asyncio.wait_for(wait(), 2)


def qualified() -> list[testing.Exchange]:
    return [*info(4, "SYNTHETIC"), *info(8, "0.0"), *read(WINDOW[0], CODE)]


def publication_open(
    original: bytes = ORIGINAL, selected: bytes = SELECTED
) -> list[testing.Exchange]:
    return [
        *qualified(),
        *read(SOURCE_STATE, original),
        *write(SOURCE_STATE, selected),
        *read(SOURCE_STATE, selected),
        *read(WINDOW[0], CODE[:4]),
        *read(SOURCE_STATE, selected),
        *data_read(WINDOW[0], CODE[:4]),
        *read(SOURCE_STATE, selected),
    ]


def data_steps(selected: bytes = SELECTED) -> tuple[list[testing.Exchange], bytes]:
    chunks = bytes(range(CAPACITY)), bytes(range(CAPACITY, 2 * CAPACITY))
    steps = [
        *read(SOURCE_STATE, selected),
        *data_read(DATA[0], chunks[0]),
        *read(SOURCE_STATE, selected),
        *read(SOURCE_STATE, selected),
        *data_read(DATA[0] + CAPACITY, chunks[1]),
        *read(SOURCE_STATE, selected),
    ]
    return steps, b"".join(chunks)


def restore_steps(original: bytes = ORIGINAL, selected: bytes = SELECTED) -> list[testing.Exchange]:
    return [
        *read(SOURCE_STATE, selected),
        *write(SOURCE_STATE, original),
        *read(SOURCE_STATE, original),
    ]


def report(memory: usb.Memory) -> dict[str, object]:
    value = memory.state
    assert isinstance(value, dict)
    return value


def loan_of(state: dict[str, object]) -> dict[str, object]:
    value = state["loan"]
    assert isinstance(value, dict)
    return value


def session_report(session: usb.Session) -> dict[str, object]:
    value = session.memory_state
    assert isinstance(value, dict)
    return value


async def test_owned_loan_reads_restores_and_blocks_the_session() -> None:
    read_steps, expected = data_steps()
    session, script = await opened(
        [
            *publication_open(),
            *read_steps,
            *restore_steps(),
            E.control(control(0xC2, 2)),
            E.read(2, b"ok"),
            *read(WINDOW[0], CODE[:4]),
        ]
    )
    memory = await session.memory(PROFILE)
    assert session.state == usb.SessionState.MemoryOwned
    borrowed = session_report(session)
    assert borrowed["phase"] == "Ready"
    assert borrowed["in_flight"] is False
    assert borrowed["loan_available"] is True
    assert borrowed["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }

    with pytest.raises(usb.SessionBusyError):
        await session.ping()
    with pytest.raises(usb.SessionBusyError):
        usb.memory.read_chunks(session, WINDOW[0], 4)
    with pytest.raises(usb.SessionBusyError):
        await session.memory(PROFILE)
    with pytest.raises(usb.SessionBusyError):
        Camera(_native.remote.Camera.synthetic_ff80(session))

    remaining = script.remaining
    with pytest.raises(ValueError, match="outside"):
        await memory.read(0, 4)
    assert script.remaining == remaining
    assert report(memory)["phase"] == "Ready"
    assert await memory.read(DATA[0], DATA[1]) == expected
    assert report(memory)["phase"] == "Ready"

    await memory.aclose()
    assert report(memory)["closed"] is True
    assert report(memory)["phase"] == "Restored"
    assert report(memory)["in_flight"] is False
    assert report(memory)["loan"] == {
        "address": SOURCE_STATE, "original": ORIGINAL, "selected": SELECTED,
    }
    assert session.state == usb.SessionState.Ready
    assert session.memory_state is None
    await memory.aclose()
    with pytest.raises(usb.ClosedSessionError):
        await memory.read(DATA[0], 4)

    assert await session.ping() == b"ok"
    async with usb.memory.read_chunks(session, WINDOW[0], 4) as reader:
        assert [chunk async for chunk in reader] == [CODE[:4]]
    assert script.remaining == 0


async def test_profiles_refuse_before_the_session_is_used() -> None:
    session, script = await opened([CANARY])
    with pytest.raises(TypeError):
        await session.memory(1234)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: a profile is text or BulkProfile.
    # An exact ID must be bundled; the profile's own firmware id is not a
    # catalog key and nothing falls back to inline facts.
    with pytest.raises(ValueError, match="bulk"):
        await session.memory("sample-1")
    with pytest.raises(ValueError):
        usb.BulkProfile.from_json("{")
    with pytest.raises(ValueError):
        usb.BulkProfile.from_json(profile_json(unknown_field=True))
    with pytest.raises(ValueError):
        usb.BulkProfile.from_json(profile_json(window={"address": WINDOW[0], "size": 0}))
    assert session.state == usb.SessionState.Ready
    assert session.memory_state is None
    assert script.remaining == 1


async def test_a_bundled_id_resolves_and_refuses_another_camera() -> None:
    root = Path(__file__).resolve().parents[3]
    text = (root / "catalog/firmware/x70/1.10/bulk.generated.json").read_text()
    firmware_id = json.loads(text)["firmware_id"]
    bundled = usb.BulkProfile.from_json(text)
    assert bundled.firmware_id == firmware_id
    session, script = await opened([*info(4, "SYNTHETIC")])
    with pytest.raises(ValueError, match="is not the selected camera"):
        await session.memory(firmware_id)
    # A known setup failure restores the pair and returns the session.
    assert session.state == usb.SessionState.Ready
    assert session.memory_state is None
    assert script.remaining == 0
    session.abandon()


async def test_canceled_setup_keeps_the_loan_and_fences_the_session() -> None:
    session, script = await opened(
        [
            *qualified(),
            *read(SOURCE_STATE, ORIGINAL),
            *write(SOURCE_STATE, SELECTED),
            *pending_memory_read(SOURCE_STATE, 8),
            CANARY,
        ]
    )
    task = asyncio.create_task(session.memory(PROFILE))
    await dispatched_until(script, 1)
    assert session.state == usb.SessionState.MemoryOwned
    in_flight = session_report(session)
    assert in_flight["phase"] == "InFlight"
    assert in_flight["in_flight"] is True
    assert in_flight["loan_available"] is False and in_flight["loan"] is None

    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    settled = session_report(session)
    assert settled["phase"] == "Unresolved"
    assert settled["in_flight"] is False
    assert settled["loan_available"] is True
    assert settled["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        await session.close()
    assert script.remaining == 1
    gc.collect()
    assert script.remaining == 1
    session.abandon()
    assert session_report(session)["phase"] == "Abandoned"
    assert session.memory_state is not None
    assert script.remaining == 1


async def test_abandon_during_open_keeps_the_report_and_never_revives() -> None:
    session, script = await opened(
        [
            *qualified(),
            *read(SOURCE_STATE, ORIGINAL),
            *write(SOURCE_STATE, SELECTED),
            *pending_memory_read(SOURCE_STATE, 8),
            CANARY,
        ]
    )
    task = asyncio.create_task(session.memory(PROFILE))
    await dispatched_until(script, 1)
    session.abandon()
    with pytest.raises(usb.ClosedSessionError):
        await task
    settled = session_report(session)
    assert settled["abandoned"] is True
    assert settled["phase"] == "Abandoned"
    assert settled["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    assert session.state == usb.SessionState.Abandoned
    with pytest.raises(usb.ClosedSessionError):
        await session.memory(PROFILE)
    assert script.remaining == 1


async def test_canceled_read_keeps_the_report_and_refuses_restore() -> None:
    session, script = await opened(
        [
            *publication_open(),
            *read(SOURCE_STATE, SELECTED),
            E.control(control(0xE1, CAPACITY, offset=DATA[0])),
            E.pending_read(CAPACITY),
            CANARY,
        ]
    )
    memory = await session.memory(PROFILE)
    task = asyncio.create_task(memory.read(DATA[0], DATA[1]))
    await dispatched_until(script, 1)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    cancelled = report(memory)
    assert cancelled["phase"] == "Unresolved"
    assert cancelled["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    with pytest.raises(usb.CompletionUnknownError):
        await memory.aclose()
    with pytest.raises(usb.CompletionUnknownError):
        await memory.read(DATA[0], 4)
    assert session.state == usb.SessionState.CompletionUnknown
    await asyncio.sleep(0)
    gc.collect()
    assert script.remaining == 1


async def test_canceled_restore_keeps_the_resource() -> None:
    session, script = await opened(
        [
            *publication_open(),
            *pending_memory_read(SOURCE_STATE, 8),
            CANARY,
        ]
    )
    memory = await session.memory(PROFILE)
    task = asyncio.create_task(memory.aclose())
    await dispatched_until(script, 1)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert report(memory)["phase"] == "Unresolved"
    assert report(memory)["closed"] is False
    with pytest.raises(usb.CompletionUnknownError):
        await memory.aclose()
    assert session.memory_state is not None
    assert script.remaining == 1


async def test_abandon_after_dropping_the_handle_keeps_the_source() -> None:
    session, script = await opened([*publication_open(), CANARY])
    memory = await session.memory(PROFILE)
    del memory
    gc.collect()
    assert session.state == usb.SessionState.MemoryOwned
    assert session_report(session)["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    with pytest.raises(usb.SessionBusyError):
        await session.ping()
    session.abandon()
    assert session.state == usb.SessionState.Abandoned
    terminal = session_report(session)
    assert terminal["abandoned"] is True and terminal["phase"] == "Abandoned"
    assert loan_of(terminal)["original"] == ORIGINAL
    with pytest.raises(usb.ClosedSessionError):
        await session.ping()
    with pytest.raises(usb.ClosedSessionError):
        await session.memory(PROFILE)
    with pytest.raises(usb.ClosedSessionError):
        session.abandon()
    assert script.remaining == 1


async def test_context_manager_restores_on_success() -> None:
    session, script = await opened([*publication_open(), *restore_steps()])
    async with await session.memory(PROFILE) as memory:
        assert report(memory)["phase"] == "Ready"
    assert session.state == usb.SessionState.Ready
    assert script.remaining == 0
    session.abandon()


async def test_failed_cleanup_adds_a_note_and_keeps_the_primary_error() -> None:
    session, script = await opened(
        [
            *publication_open(),
            *read(SOURCE_STATE, SELECTED),
            E.control_disconnected(control(0xD0, 36 + len(ORIGINAL))),
            CANARY,
        ]
    )
    with pytest.raises(ValueError, match="body failed") as caught:
        async with await session.memory(PROFILE) as memory:
            assert report(memory)["phase"] == "Ready"
            raise ValueError("body failed")
    notes = getattr(caught.value, "__notes__", [])
    assert any("memory cleanup failed" in note for note in notes), notes
    assert session.memory_state is not None
    assert session_report(session)["phase"] == "Unresolved"
    await asyncio.sleep(0)
    gc.collect()
    assert script.remaining == 1
