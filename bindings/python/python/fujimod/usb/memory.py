"""Compositions delegated to the Rust memory operations."""

from collections.abc import Buffer

from .._bridge import buffer_in, buffer_out, call, jsusb, uint, unwrap
from . import Reader, Session


async def read_range(session: Session, address: int, length: int) -> bytes:
    return await session._call(
        jsusb.memory.readRange,
        buffer_out,
        unwrap(session, Session),
        uint(address),
        uint(length),
    )


async def write_range(session: Session, address: int, data: Buffer) -> None:
    await session._call(
        jsusb.memory.writeRange,
        (lambda value: value),
        unwrap(session, Session),
        uint(address),
        buffer_in(data),
    )


def read_chunks(session: Session, address: int, length: int) -> Reader:
    return Reader._wrap(
        call(
            jsusb.memory.readChunks,
            unwrap(session, Session),
            uint(address),
            uint(length),
        )
    )
