"""Compositions delegated to the Rust diagnostic-data operations."""

from collections.abc import Buffer
from dataclasses import dataclass
from typing import TYPE_CHECKING, Self

from .._bridge import (
    buffer_in,
    buffer_out,
    call,
    enum_in,
    jsusb,
    snapshot,
    uint,
    unwrap,
)
from . import DataWindow, DataWriteMode, DiagnosticReply, Reader, Session

if TYPE_CHECKING:
    from _fujimod_js import JsUploadReply as JsUploadReply


@dataclass(frozen=True)
class UploadReply:
    setup: DiagnosticReply
    acknowledgment: bytes
    bytes_written: int

    @classmethod
    def _from_js(cls, value: JsUploadReply) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                DiagnosticReply._from_js(v.setup),
                buffer_out(v.acknowledgment),
                v.bytesWritten,
            ),
        )


async def upload(
    session: Session, data: Buffer, *, piece_bytes: int, mode: DataWriteMode
) -> UploadReply:
    return await session._call(
        jsusb.data.upload,
        UploadReply._from_js,
        unwrap(session, Session),
        buffer_in(data),
        uint(piece_bytes),
        enum_in(mode, DataWriteMode),
    )


def read_chunks(
    session: Session, window: DataWindow, address: int, length: int
) -> Reader:
    return Reader._wrap(
        call(
            jsusb.data.readChunks,
            unwrap(session, Session),
            unwrap(window, DataWindow),
            uint(address),
            uint(length),
        )
    )
