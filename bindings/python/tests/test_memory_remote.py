"""Remote cameras over an owned bulk-memory loan.

The memory, ranges and pixel-free exchanges below are synthetic: the profile is
a tiny publication-source bulk document whose window, source pair and stock
bytes are test data. The session, mapping, provider and remote adapter are the
production code, and the transport is `fujimod.usb.testing` replay.

The remote facts come from one public catalog selection and supply identity,
alias and control metadata. All requested bytes are synthetic, including the
out-of-window reads that check the bounded ordinary path.
"""

from __future__ import annotations

import asyncio
import gc
import hashlib
import struct
from functools import cache

import pytest
from fujimod import usb
from fujimod.remote import Camera, control
from fujimod.usb import testing
from test_memory import (
    CANARY,
    control as envelope,
    data_read,
    exchange,
    info,
    opened,
    pending_memory_read,
    profile_json,
    read,
    write,
)

E = testing.Exchange
FW_ID = "x70-110"
WINDOW = (0x40001000, 0x1000)
SOURCE_STATE = 0x40001200
CAPACITY = 32
DATA = (0x40001800, 0x80)
UNALIGNED = (0x40001804, 0x80)
OUTSIDE = 0x40003800
CODE = b"synthetic bulk code"
ORIGINAL = struct.pack("<2I", 0x0BAD0000, 16)
SELECTED = struct.pack("<2I", 0, CAPACITY)


@cache
def remote_facts() -> dict:
    from fujimod.emulator import catalog
    from fujimod.remote import facts_from_selection

    return facts_from_selection(catalog.resolve_firmware(FW_ID))


def profile(firmware_id: str = FW_ID, *, canonical: bool = False) -> usb.BulkProfile:
    """One synthetic publication profile, aligned to its DMA declaration."""
    alias = remote_facts()['firmware']['uncached_alias'] if canonical else 0
    base, data_address = WINDOW[0] & ~alias, DATA[0] & ~alias
    return usb.BulkProfile.from_json(
        profile_json(
            firmware_id=firmware_id,
            window={"address": base, "size": WINDOW[1]},
            source={
                "mechanism": "publication",
                "source_state": SOURCE_STATE,
                "capacity": CAPACITY,
            },
            dma={"address_alignment": 8, "length_granule": 8},
            code_checks=[
                {
                    "name": "entry",
                    "address": base,
                    "size": len(CODE),
                    "sha256": hashlib.sha256(CODE).hexdigest(),
                }
            ],
            probes=[{"address": base, "size": 8}],
            scopes=[
                {
                    "name": "quick",
                    "basis": "Synthetic test range.",
                    "ranges": [{"name": "data", "address": data_address, "size": DATA[1]}],
                }
            ],
        )
    )


PROFILE = profile()


def qualified(base: int = WINDOW[0]) -> list[testing.Exchange]:
    return [*info(4, "SYNTHETIC"), *info(8, "0.0"), *read(base, CODE)]


def publication_open(base: int = WINDOW[0]) -> list[testing.Exchange]:
    return [
        *qualified(base),
        *read(SOURCE_STATE, ORIGINAL),
        *write(SOURCE_STATE, SELECTED),
        *read(SOURCE_STATE, SELECTED),
        *read(base, CODE[:8]),
        *read(SOURCE_STATE, SELECTED),
        *data_read(base, CODE[:8]),
        *read(SOURCE_STATE, SELECTED),
    ]


def restore_steps() -> list[testing.Exchange]:
    return [
        *read(SOURCE_STATE, SELECTED),
        *write(SOURCE_STATE, ORIGINAL),
        *read(SOURCE_STATE, ORIGINAL),
    ]


def piece(address: int, size: int) -> bytes:
    return bytes(((address + index) % 256) for index in range(size))


def bulk_steps(start: int, total: int) -> tuple[list[testing.Exchange], bytes]:
    """An admitted E1 read: owner check, data chunk, owner check per piece."""
    steps: list[testing.Exchange] = []
    data = b""
    for offset in range(0, total, CAPACITY):
        chunk = piece(start + offset, CAPACITY)
        steps += [
            *read(SOURCE_STATE, SELECTED),
            *data_read(start + offset, chunk),
            *read(SOURCE_STATE, SELECTED),
        ]
        data += chunk
    return steps, data


def raw_steps(start: int, size: int, limit: int = 64) -> list[testing.Exchange]:
    """A slow read_range: one bounded command per limit-sized part."""
    steps: list[testing.Exchange] = []
    for offset in range(0, size, limit):
        part = min(limit, size - offset)
        steps += read(start + offset, piece(start + offset, part))
    return steps


PING = [E.control(envelope(0xC2, 2)), E.read(2, b"ok")]
CLOSE = [E.control(envelope(0xC4, 2)), E.read(2, b"ok")]


def command(function: int, parameters: bytes, payload: bytes = b"") -> bytes:
    request = bytearray(36)
    struct.pack_into("<I", request, 0, 0x200001)
    struct.pack_into("<I", request, 16, function)
    request[20:36] = parameters
    return bytes(request) + payload


def event_steps(payload: bytes) -> list[testing.Exchange]:
    parameters = bytearray(16)
    struct.pack_into("<I", parameters, 12, len(payload))
    return exchange(command(5, parameters, payload), command(5, parameters))


def pending_event_steps(payload: bytes) -> list[testing.Exchange]:
    """One event command whose reply never arrives: the command is in flight."""
    parameters = bytearray(16)
    struct.pack_into("<I", parameters, 12, len(payload))
    request = command(5, parameters, payload)
    reply = command(5, parameters)
    return [
        E.control(envelope(0xD0, len(request))),
        E.write(request),
        E.control(envelope(0xD1, len(reply))),
        E.pending_read(len(reply)),
    ]


def deadline(camera: Camera) -> int:
    return camera.now + 1_000_000_000


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


def camera(memory: usb.Memory) -> Camera:
    facts = remote_facts()
    assert facts["firmware"]["firmware_id"] == FW_ID
    return Camera.from_memory(memory, facts, event_bytes=64)


async def test_admitted_bulk_read_slow_fallbacks_and_event_share_the_loan() -> None:
    steps, expected = bulk_steps(DATA[0], DATA[1])
    session, script = await opened(
        [
            *publication_open(),
            *steps,
            *raw_steps(UNALIGNED[0], UNALIGNED[1]),
            *read(OUTSIDE, b"\x11\x22\x33\x44"),
            *event_steps(b"FUNCTION OFF\n"),
            *restore_steps(),
            *PING,
            *CLOSE,
        ]
    )
    memory = await session.memory(PROFILE)
    assert session.state == usb.SessionState.MemoryOwned
    assert loan_of(session_report(session)) == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    remote = camera(memory)

    whole = await remote.wait_until_memory(
        lambda values: True, [DATA], deadline=deadline(remote), condition="bulk"
    )
    assert whole == [(DATA[0], expected)]
    assert report(memory)["phase"] == "Ready"

    # Larger than the slow-read limit but outside the DMA declaration: the
    # ordinary bounded reader serves it in limit-sized commands, not E1.
    unaligned = await remote.wait_until_memory(
        lambda values: True,
        [UNALIGNED],
        deadline=deadline(remote),
        condition="unaligned",
    )
    assert unaligned == [(UNALIGNED[0], piece(UNALIGNED[0], UNALIGNED[1]))]

    outside = await remote.wait_until_memory(
        lambda values: True,
        [(OUTSIDE, 4)],
        deadline=deadline(remote),
        condition="outside",
    )
    assert outside == [(OUTSIDE, b"\x11\x22\x33\x44")]

    # A released control sends its OFF event on the retained session.
    await remote.set_input(control("wifi_button"), "released", deadline=deadline(remote))
    assert report(memory)["phase"] == "Ready"

    # Closing the adapter restores nothing: the loan still owns the session.
    remote.close()
    with pytest.raises(usb.SessionBusyError):
        await session.ping()

    await memory.aclose()
    assert report(memory)["closed"] is True
    assert report(memory)["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    assert session.state == usb.SessionState.Ready
    assert session.memory_state is None
    assert await session.ping() == b"ok"
    await session.close()
    assert script.remaining == 0


async def test_a_profile_for_another_firmware_refuses_before_traffic() -> None:
    session, script = await opened([*publication_open(), CANARY])
    memory = await session.memory(profile("other-1"))
    with pytest.raises(ValueError, match="not x70-110"):
        camera(memory)
    assert script.remaining == 1
    assert session.state == usb.SessionState.MemoryOwned
    session.abandon()
    assert script.remaining == 1


async def test_remote_alias_selects_only_the_admitted_canonical_bulk_window() -> None:
    alias = remote_facts()['firmware']['uncached_alias']
    base, address = WINDOW[0] & ~alias, DATA[0] & ~alias
    steps, expected = bulk_steps(address, DATA[1])
    session, script = await opened([
        *publication_open(base), *steps, *read(OUTSIDE, b"abcd"),
        *restore_steps(), CANARY,
    ])
    memory = await session.memory(profile(canonical=True))
    remote = camera(memory)
    observed = await remote.wait_until_memory(
        lambda rows: True, [(address, DATA[1])],
        deadline=deadline(remote), condition='canonical bulk window',
    )
    assert observed == [(address, expected)]
    outside = OUTSIDE & ~alias
    observed = await remote.wait_until_memory(
        lambda rows: True, [(outside, 4)],
        deadline=deadline(remote), condition='outside canonical window',
    )
    assert observed == [(outside, b"abcd")]
    remote.close()
    await memory.aclose()
    assert script.remaining == 1
    session.abandon()


async def test_a_non_memory_object_refuses_before_traffic() -> None:
    session, script = await opened([CANARY])
    with pytest.raises(TypeError):
        Camera.from_memory(object(), remote_facts(), event_bytes=64)  # ty: ignore[invalid-argument-type]
    assert script.remaining == 1
    session.abandon()


async def test_a_closed_memory_refuses_before_traffic() -> None:
    session, script = await opened([*publication_open(), *restore_steps(), CANARY])
    memory = await session.memory(PROFILE)
    await memory.aclose()
    with pytest.raises(usb.ClosedSessionError):
        camera(memory)
    assert script.remaining == 1
    assert session.state == usb.SessionState.Ready
    session.abandon()
    assert script.remaining == 1


async def assert_canceled(script: testing.Script, task: asyncio.Task[object],
                          memory: usb.Memory, session: usb.Session) -> None:
    """Cancel after observed dispatch, then check the fenced, retained loan."""
    await dispatched(script)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    state = report(memory)
    assert state["phase"] == "Unresolved"
    assert state["loan"] == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    assert session_report(session) is not None
    assert session.state == usb.SessionState.CompletionUnknown
    with pytest.raises(usb.CompletionUnknownError):
        await memory.aclose()


async def dispatched(script: testing.Script) -> None:
    async def wait() -> None:
        while script.remaining > 1:
            await asyncio.sleep(0)

    await asyncio.wait_for(wait(), 2)


async def test_canceled_bulk_read_fences_the_loan_without_cleanup() -> None:
    session, script = await opened(
        [
            *publication_open(),
            *read(SOURCE_STATE, SELECTED),
            E.control(envelope(0xE1, CAPACITY, offset=DATA[0])),
            E.pending_read(CAPACITY),
            CANARY,
        ]
    )
    memory = await session.memory(PROFILE)
    remote = camera(memory)
    task = asyncio.create_task(
        remote.wait_until_memory(
            lambda values: True, [DATA], deadline=deadline(remote), condition="bulk"
        )
    )
    await assert_canceled(script, task, memory, session)
    remote.close()
    await asyncio.sleep(0)
    gc.collect()
    assert script.remaining == 1
    session.abandon()
    assert script.remaining == 1


async def test_canceled_raw_read_fences_the_loan_without_cleanup() -> None:
    session, script = await opened(
        [*publication_open(), *pending_memory_read(OUTSIDE, 4), CANARY]
    )
    memory = await session.memory(PROFILE)
    remote = camera(memory)
    task = asyncio.create_task(
        remote.wait_until_memory(
            lambda values: True,
            [(OUTSIDE, 4)],
            deadline=deadline(remote),
            condition="outside",
        )
    )
    await assert_canceled(script, task, memory, session)
    remote.close()
    assert script.remaining == 1
    session.abandon()
    assert script.remaining == 1


async def test_canceled_event_fences_the_loan_without_cleanup() -> None:
    session, script = await opened(
        [*publication_open(), *pending_event_steps(b"FUNCTION OFF\n"), CANARY]
    )
    memory = await session.memory(PROFILE)
    remote = camera(memory)
    task = asyncio.create_task(
        remote.set_input(control("wifi_button"), "released", deadline=deadline(remote))
    )
    await assert_canceled(script, task, memory, session)
    remote.close()
    assert script.remaining == 1
    session.abandon()
    assert script.remaining == 1


async def test_dropping_camera_and_memory_keeps_the_loan_excluded() -> None:
    session, script = await opened([*publication_open(), CANARY])
    memory = await session.memory(PROFILE)
    remote = camera(memory)
    del remote, memory
    gc.collect()
    assert session.state == usb.SessionState.MemoryOwned
    assert loan_of(session_report(session)) == {
        "address": SOURCE_STATE,
        "original": ORIGINAL,
        "selected": SELECTED,
    }
    with pytest.raises(usb.SessionBusyError):
        await session.ping()
    # Abandon is terminal and performs no source cleanup.
    session.abandon()
    terminal = session_report(session)
    assert terminal["abandoned"] is True
    assert terminal["phase"] == "Abandoned"
    assert loan_of(terminal)["original"] == ORIGINAL
    with pytest.raises(usb.ClosedSessionError):
        await session.ping()
    assert script.remaining == 1


async def test_memory_aclose_while_the_camera_is_idle_closes_the_owner_once() -> None:
    session, script = await opened([*publication_open(), *restore_steps()])
    memory = await session.memory(PROFILE)
    remote = camera(memory)
    await memory.aclose()
    assert report(memory)["closed"] is True
    assert report(memory)["phase"] == "Restored"
    assert session.state == usb.SessionState.Ready
    with pytest.raises(RuntimeError, match="closed"):
        await remote.wait_until_memory(
            lambda values: True,
            [(DATA[0], 4)],
            deadline=deadline(remote),
            condition="closed",
        )
    # A second close is a no-op and sends no traffic.
    await memory.aclose()
    assert script.remaining == 0
    remote.close()
    session.abandon()
    assert script.remaining == 0
