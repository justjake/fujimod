"""Runtime leases and control publication over the native service session."""

import json
import struct

import pytest
from fujimod import usb
from fujimod.runtime_install import Installer, RuntimeInstallError
from fujimod.usb import memory, research, testing

LIMITS = usb.FirmwareLimits(memory_read=64, memory_write=64, configuration=1, console=1)


def image() -> usb.RuntimeImage:
    data = bytearray(256)
    struct.pack_into("<2I", data, 64, 0x12345678, 7)
    return usb.RuntimeImage(
        data,
        json.dumps(
            dict(
                version=1,
                link_base=0x1000,
                alignment=32,
                capacity=256,
                hooks=[dict(address=0x3000, old=1, new=2)],
                relocations=[],
                hook_table=None,
                control=dict(
                    control_offset=0,
                    state_offset=64,
                    magic_offset=0,
                    nonce_offset=4,
                    token_offset=8,
                    operation_offset=None,
                    inits_done_offset=12,
                    installed_offset=16,
                    magic=0x12345678,
                    nonce=7,
                    core=3,
                    init_count=2,
                    operations=dict(install=1, rollback=2, status=3, init=4),
                    read_alias=0,
                    code_size=64,
                    mailbox=dict(
                        offset=32,
                        size=36,
                        nonce=0,
                        operation=4,
                        status=8,
                        result=12,
                        words_new=16,
                        words_old=20,
                        core=24,
                        token=28,
                        inits_done=32,
                    ),
                ),
            )
        ),
    )


def lease(installer: Installer) -> dict[str, object]:
    value = installer.lease
    assert value is not None
    return value


class Peer:
    def __init__(self) -> None:
        self.ram = {0x3000: 1}
        self.pending = b""
        self.operation = 0
        self.replay = False

    def get(self, address: int, size: int) -> bytes:
        return bytes(self.ram.get(address + i, 0) for i in range(size))

    def put(self, address: int, data: bytes) -> None:
        self.ram.update((address + i, value) for i, value in enumerate(data))

    def control(self, envelope: bytes) -> None:
        self.operation = envelope[0]
        if self.operation == 0xC0:
            self.pending = b"\x2a\0\0\0\0\0\0\x40"

    def write(self, raw: bytes) -> None:
        (code,) = struct.unpack_from("<I", raw)
        if code == 0x200001:
            function, _, _, address, size = struct.unpack_from("<5I", raw, 16)
            if function == 2:
                self.pending = raw[:36] + self.get(address, size)
            elif function == 3:
                self.put(address, raw[36:])
                self.pending = raw[:36]
            else:
                raise AssertionError(function)
        elif code == 0x450006:
            (op,) = struct.unpack_from("<I", raw, 16)
            token = int.from_bytes(self.get(0x2048, 4), "little")
            if op in (1, 2):
                self.put(0x2050, (1 if op == 1 else 0).to_bytes(4, "little"))
                self.put(0x3000, (2 if op == 1 else 1).to_bytes(4, "little"))
            if op == 4:
                self.put(0x204C, (1).to_bytes(4, "little"))
            self.put(
                0x2060,
                struct.pack(
                    "<9I",
                    7,
                    op,
                    0,
                    2 if op == 4 else 0,
                    0,
                    0,
                    3,
                    token + int(self.replay),
                    1,
                ),
            )
            self.pending = struct.pack("<8I", code, 0, 0, 0, 0, 0, 0, 0)
        else:
            raise AssertionError(code)

    def read(self, maximum: int) -> bytes:
        assert len(self.pending) <= maximum
        return self.pending


class Body:
    def __init__(self, session: usb.Session) -> None:
        self.session = session
        self.events = []
        self.failure = None

    async def allocate(self, size: int, align: int) -> int:
        self.events.append("allocate")
        assert size == 256 and align == 32
        return 0x2000

    async def free(self, base: int) -> None:
        self.events.append("free")
        assert base == 0x2000

    async def placed(self, image: usb.RuntimeImage, lease: dict[str, object]) -> None:
        self.events.append("placed")
        assert lease["base"] == 0x2000

    async def read(self, address: int, size: int) -> bytes:
        return await memory.read_range(self.session, address, size)

    async def write(self, address: int, data: bytes) -> None:
        if self.failure == "write":
            raise ValueError("upload failed")
        await memory.write_range(self.session, address, data)

    async def read_many(self, ranges: list[tuple[int, int]]) -> list[bytes]:
        return [await self.read(address, size) for address, size in ranges]

    async def write_many(self, writes: list[tuple[int, bytes]]) -> None:
        for address, data in writes:
            await self.write(address, data)

    async def synchronize(self, address: int, size: int) -> None:
        self.events.append("synchronize")

    async def protect(self, address: int, size: int) -> None:
        self.events.append("protect")

    async def control(self, entry: int, operation: int) -> None:
        self.events.append("control")
        if self.failure == "control":
            raise ValueError("control failed")
        body = struct.pack("<4I", operation, 0, 0, 0)
        request = research.Request(
            research.CommandCode(0x450006), body, reply_capacity=32
        )
        plan = usb.CallbackInvocation(
            0x400,
            bytes(4),
            entry.to_bytes(4, "little"),
            request,
            LIMITS,
            replies=[struct.pack("<8I", 0x450006, 0, 0, 0, 0, 0, 0, 0)],
        )
        await self.session.invoke_callback(plan)


@pytest.mark.parametrize("failure", [None, "write", "control"])
async def test_install_and_failure_retention(failure: str | None) -> None:
    peer = Peer()
    session = await testing.peer_session(peer, LIMITS, ep_in=0x82, ep_out=1)
    body = Body(session)
    body.failure = failure
    installer = Installer(image(), body)
    try:
        if failure:
            with pytest.raises(RuntimeInstallError, match="failed"):
                await installer.install(11)
            assert ("free" in body.events) == (failure == "write")
            assert lease(installer)["publication_attempted"] == (failure == "control")
        else:
            await installer.install(11)
            assert lease(installer)["installed"]
            await installer.status(13)
            await installer.set(0x20C0, b"data")
            assert await installer.read(0x20C0, 4) == b"data"
            await installer.rollback(14)
            assert not lease(installer)["installed"]
            assert (
                lease(installer)["publication_attempted"] and "free" not in body.events
            )
            assert peer.get(0x400, 4) == bytes(4)
    finally:
        session.abandon()


async def test_stale_mailbox_keeps_publication_lease() -> None:
    peer = Peer()
    peer.replay = True
    session = await testing.peer_session(peer, LIMITS, ep_in=0x82, ep_out=1)
    body = Body(session)
    installer = Installer(image(), body)
    try:
        with pytest.raises(RuntimeInstallError, match="mailbox"):
            await installer.install(11)
        assert lease(installer)["publication_attempted"] and "free" not in body.events
    finally:
        session.abandon()


async def test_preparation_upload_and_readback_use_script() -> None:
    from .test_usb_callback import command, control, read

    E = testing.Exchange
    placed = image().relocate(0x2000)
    data = bytes(placed.data)
    steps = [control(0xC0, 8, 0), E.read(8, b"\x2a\0\0\0\0\0\0\x40")]
    for offset in range(0, len(data), 64):
        piece = data[offset : offset + 64]
        header = struct.pack(
            "<9I", 0x200001, 0, 0, 0, 3, 0, 0, 0x2000 + offset, len(piece)
        )
        steps += command(header + piece, header)
    for offset in range(0, len(data), 64):
        steps += read(0x2000 + offset, data[offset : offset + 64])
    session, script = await testing.session(steps, LIMITS)
    body = Body(session)
    installer = Installer(image(), body)
    try:
        await installer.prepare()
        assert script.remaining == 0
        assert not lease(installer)["publication_attempted"]
        assert body.events == ["allocate", "placed", "synchronize", "protect"]
    finally:
        session.abandon()


@pytest.mark.parametrize("fail", [False, True])
async def test_python_relink_callback_receives_allocated_base(fail: bool) -> None:
    peer = Peer()
    session = await testing.peer_session(peer, LIMITS, ep_in=0x82, ep_out=1)
    bases = []

    def rebuild(base: int) -> usb.RuntimeImage:
        bases.append(base)
        if fail:
            raise ValueError("relink failed")
        return image().relocate(base)

    body = Body(session)
    installer = Installer(image(), body, relink=rebuild)
    try:
        if fail:
            with pytest.raises(RuntimeInstallError, match="relink failed"):
                await installer.install(11)
            assert "free" in body.events and lease(installer)["freed"]
        else:
            await installer.install(11)
            assert lease(installer)["installed"]
        assert bases == [0x2000]
    finally:
        session.abandon()
