"""Rust runtime installer with an awaitable, caller-supplied Body.

Body methods: allocate(size, align) -> base; free(base); read(address, size)
-> bytes; write(address, data); read_many([(address, size), ...]) -> [bytes];
write_many([(address, data), ...]); synchronize(address, size);
protect(address, size); control(entry, operation); placed(image, lease).
read_many and write_many should each wait for the device once and write_many
must not read back: the installer verifies the resulting state itself. The placed notification
lets callers persist the image and allocation before the first upload.
"""

import json
from collections.abc import Buffer, Callable, Sequence
from typing import Protocol, cast

from . import usb


class Body(Protocol):
    """Caller-supplied memory operations used by the Rust installer."""

    async def allocate(self, size: int, align: int) -> int: ...
    async def free(self, base: int) -> None: ...
    async def placed(
        self, image: usb.RuntimeImage, lease: dict[str, object]
    ) -> None: ...
    async def read(self, address: int, size: int) -> bytes: ...
    async def write(self, address: int, data: bytes) -> None: ...
    async def read_many(self, ranges: list[tuple[int, int]]) -> Sequence[bytes]: ...
    async def write_many(self, writes: list[tuple[int, bytes]]) -> None: ...
    async def synchronize(self, address: int, size: int) -> None: ...
    async def protect(self, address: int, size: int) -> None: ...
    async def control(self, entry: int, operation: int) -> None: ...


def _object(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TypeError("Rust installer facts must be a JSON object")
    return cast(dict[str, object], value)


def _objects(value: object) -> list[dict[str, object]]:
    if not isinstance(value, list):
        raise TypeError("Rust installer facts must be a JSON array")
    return [_object(item) for item in cast(list[object], value)]


def _number(value: object) -> int:
    return int(cast(int, value))


def _lease(value: object) -> dict[str, object] | None:
    return None if value is None else _object(value)


class RuntimeInstallError(RuntimeError):
    """The installer retains its lease, including uncertain publication."""


class Installer:
    def __init__(
        self,
        image: usb.RuntimeImage,
        body: Body,
        *,
        lease: dict[str, object] | None = None,
        relink: Callable[[int], usb.RuntimeImage] | None = None,
    ) -> None:
        self.image, self.body, self.lease = image, body, lease
        self.relink = relink

    @classmethod
    def plan(
        cls,
        data: Buffer,
        metadata: dict[str, object],
        body: Body,
        *,
        relink: Callable[[int], usb.RuntimeImage] | None = None,
    ) -> Installer:
        return cls(usb.RuntimeImage(data, json.dumps(metadata)), body, relink=relink)

    async def _run(
        self,
        operation: str,
        *,
        token: int = 1,
        address: int = 0,
        size: int = 0,
        data: Buffer = b"",
    ) -> object:
        task = usb.RuntimeTask(
            self.image,
            operation,
            lease=json.dumps(self.lease) if self.lease else None,
            token=token,
            address=address,
            size=size,
            data=data,
            relink=self.relink,
        )
        response: Buffer | None = None
        failure: str | None = None
        cause: BaseException | None = None
        while True:
            kind, metadata, data = task.step(response, failure)
            facts = _object(json.loads(metadata))
            response = failure = None
            if kind == "done":
                self.lease = _lease(facts["lease"])
                if operation != "read":
                    self.image = usb.RuntimeImage(data, json.dumps(facts["image"]))
                if "error" in facts:
                    raise RuntimeInstallError(facts["error"]) from cause
                return bytes(data) if operation == "read" else facts["output"]
            try:
                if kind == "allocate":
                    base = await self.body.allocate(
                        _number(facts["size"]), _number(facts["align"])
                    )
                    response = base.to_bytes(8, "little")
                elif kind == "placed":
                    self.image = usb.RuntimeImage(data, json.dumps(facts["image"]))
                    self.lease = _lease(facts["lease"])
                    assert self.lease is not None
                    await self.body.placed(self.image, self.lease)
                elif kind == "free":
                    await self.body.free(_number(facts["base"]))
                elif kind == "read":
                    response = bytes(
                        await self.body.read(
                            _number(facts["address"]), _number(facts["size"])
                        )
                    )
                elif kind == "write":
                    await self.body.write(_number(facts["address"]), bytes(data))
                elif kind == "read_many":
                    ranges = [
                        (_number(r["address"]), _number(r["size"]))
                        for r in _objects(facts["ranges"])
                    ]
                    parts = [bytes(part) for part in await self.body.read_many(ranges)]
                    if [len(part) for part in parts] != [size for _, size in ranges]:
                        raise RuntimeError("read_many returned the wrong sizes")
                    response = b"".join(parts)
                elif kind == "write_many":
                    writes, offset = [], 0
                    for r in _objects(facts["ranges"]):
                        length = _number(r["size"])
                        writes.append((
                            _number(r["address"]),
                            bytes(data[offset : offset + length]),
                        ))
                        offset += length
                    await self.body.write_many(writes)
                elif kind in ("synchronize", "protect"):
                    address, size = _number(facts["address"]), _number(facts["size"])
                    if kind == "synchronize":
                        await self.body.synchronize(address, size)
                    else:
                        await self.body.protect(address, size)
                elif kind == "control":
                    image = _object(json.loads(self.image.metadata))
                    if (
                        facts["operation"]
                        == _object(_object(image["control"])["operations"])["install"]
                    ):
                        assert self.lease is not None
                        self.lease["publication_attempted"] = True
                    await self.body.control(
                        _number(facts["entry"]), _number(facts["operation"])
                    )
                else:
                    raise RuntimeError(f"Unknown Rust body operation: {kind}")
            except Exception as error:
                cause, failure = error, str(error)

    async def prepare(self) -> object:
        return await self._run("prepare")

    async def install(self, token: int = 1) -> object:
        return await self._run("install", token=token)

    async def publish(self, token: int = 1) -> object:
        return await self._run("publish", token=token)

    async def init(self, token: int = 1) -> object:
        return await self._run("init", token=token)

    async def status(self, token: int = 1) -> object:
        return await self._run("status", token=token)

    async def teardown(self, token: int = 1) -> object:
        return await self._run("teardown", token=token)

    async def rollback(self, token: int = 1) -> object:
        return await self._run("rollback", token=token)

    async def release(self, token: int = 1) -> object:
        """Free an image whose hooks are stock and which holds nothing."""
        return await self._run("release", token=token)

    async def set(self, address: int, data: Buffer) -> object:
        return await self._run("set", address=address, data=bytes(data))

    async def read(self, address: int, size: int) -> object:
        return await self._run("read", address=address, size=size)
