"""Compositions delegated to the Rust memory operations."""

from collections.abc import Buffer

from . import DataWindow, OffsetMode, Reader, Session
from .data import read_chunks as _read_data_chunks


async def read_range(session: Session, address: int, length: int) -> bytes:
    return await session.read_memory(address, length)


async def write_range(session: Session, address: int, data: Buffer) -> None:
    await session.write_memory(address, data)


def read_chunks(session: Session, address: int, length: int) -> Reader:
    """Stream a range through the session's diagnostic data window."""
    window = DataWindow(address, address, length, length, OffsetMode.Checked)
    return _read_data_chunks(session, window, 0, length)
