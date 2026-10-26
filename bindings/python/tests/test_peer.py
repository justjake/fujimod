"""Stateful virtual endpoints run through the same Rust engine as USB devices."""

import struct
from typing import TypedDict, Unpack

import pytest
from fujimod import usb
from fujimod.usb import testing, tracing


class testingOptions(TypedDict, total=False):
    ep_in: int
    ep_out: int
    max_reply_bytes: int


class PeerOptions(TypedDict, total=False):
    max_reply_bytes: int
    trace: tracing.Trace | None


LIMITS = usb.FirmwareLimits(
    memory_read=256, memory_write=256, configuration=256, console=256
)


class Peer:
    def __init__(self) -> None:
        self.calls = []
        self.pending = b""
        self.fail_write: BaseException | None = None
        self.wrong_result: object = None

    def control(self, data: bytes) -> object:
        assert type(data) is bytes
        self.calls.append(("control", data))
        kind, unused, offset, size, token = struct.unpack(">BBIIH", data)
        assert unused == 0
        if kind == 0xC0:
            assert token == 0 and size == 8
            self.pending = b"\x34\x12\0\0\0\0\x01\0"
        else:
            assert token == 0x1234
            if kind in (0xC2, 0xC4):
                self.pending = b"\x34\x12"
            elif kind == 0xE1:
                self.pending = bytes((offset + i) % 256 for i in range(size))
            else:
                assert kind in (0xD0, 0xD1)

    def write(self, data: bytes) -> object:
        assert type(data) is bytes
        self.calls.append(("write", data))
        if self.fail_write is not None:
            raise self.fail_write
        if self.wrong_result is not None:
            return self.wrong_result
        command, _, _, _, function, _, _, address, size = struct.unpack("<9I", data)
        assert command == 0x200001 and function == 2
        self.pending = data + bytes((address + i) % 256 for i in range(size))

    def read(self, maximum: int) -> object:
        self.calls.append(("read", maximum))
        data, self.pending = self.pending[:maximum], self.pending[maximum:]
        return data


async def session(peer: Peer, **kwargs: Unpack[PeerOptions]) -> usb.Session:
    return await testing.peer_session(peer, LIMITS, ep_in=0x82, ep_out=1, **kwargs)


async def test_virtual_peer_uses_production_memory_and_trace_with_selected_endpoints() -> (
    None
):
    peer = Peer()
    trace = tracing.Trace(64, 4096)
    native = await session(peer, trace=trace)
    assert (await usb.memory.read_range(native, 0x1000, 260)) == bytes(
        range(256)
    ) + bytes(range(4))
    assert (await native.ping()) == b"\x34\x12"
    assert (await native.close()) == b"\x34\x12"
    assert native.state == usb.SessionState.Closed
    batch = trace.drain()
    assert batch.dropped_events == 0
    assert {e.endpoint for e in batch.events} == {0, 0x82, 1}
    assert all(
        e.phase in (tracing.Phase.Submitted, tracing.Phase.Completed)
        for e in batch.events
    )
    assert [
        struct.unpack_from("<I", data, 32)[0]
        for kind, data in peer.calls
        if kind == "write"
    ] == [256, 4]


@pytest.mark.parametrize(
    "error",
    [RuntimeError("virtual scheduler stopped"), KeyboardInterrupt("virtual interrupt")],
)
async def test_callback_exception_identity_is_preserved_and_session_is_fenced(
    error: BaseException,
) -> None:
    peer = Peer()
    native = await session(peer)
    peer.fail_write = error
    with pytest.raises(type(error)) as failed:
        (await native.read_memory(4, 4))
    assert failed.value is error
    assert native.state == usb.SessionState.CompletionUnknown
    calls = len(peer.calls)
    with pytest.raises(usb.CompletionUnknownError):
        (await native.close())
    with pytest.raises(usb.CompletionUnknownError):
        (await native.ping())
    assert len(peer.calls) == calls


@pytest.mark.parametrize("value", [0, 1, True, b"not a completion"])
async def test_control_and_write_callbacks_cannot_return_magic_integers(
    value: object,
) -> None:
    class BadControl(Peer):
        def control(self, data: bytes) -> object:
            return value

    with pytest.raises(TypeError, match="return None"):
        (await session(BadControl()))
    peer = Peer()
    native = await session(peer)
    peer.wrong_result = value
    with pytest.raises(TypeError, match="return None"):
        (await native.read_memory(4, 4))
    assert native.state == usb.SessionState.CompletionUnknown


async def test_read_callback_requires_bytes_and_invalid_reply_uses_core_validation() -> (
    None
):
    class WrongType(Peer):
        def read(self, maximum: int) -> object:
            raw = super().read(maximum)
            assert isinstance(raw, bytes)
            return bytearray(raw)

    with pytest.raises(TypeError):
        (await session(WrongType()))
    peer = Peer()
    native = await session(peer)
    peer.control = lambda data: None  # ty: ignore[invalid-assignment]  # Replace the callback to test a malformed read reply.
    peer.pending = b"x"
    with pytest.raises(usb.ProtocolError):
        (await native.ping())
    assert native.state == usb.SessionState.CompletionUnknown


@pytest.mark.parametrize(
    "options",
    [
        {"ep_in": 1, "ep_out": 2},
        {"ep_in": 0x81, "ep_out": 0x82},
        {"ep_in": 0x81, "ep_out": 2, "max_reply_bytes": 0},
    ],
)
async def test_invalid_endpoint_or_budget_never_calls_the_peer(
    options: testingOptions,
) -> None:
    peer = Peer()
    with pytest.raises(ValueError):
        (await testing.peer_session(peer, LIMITS, **options))
    assert peer.calls == []
