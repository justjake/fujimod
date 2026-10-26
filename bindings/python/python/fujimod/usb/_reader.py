"""Python async iterator lifetime over Rust's owned range reader."""

import asyncio
from typing import TYPE_CHECKING, Literal, Self

from .._bridge import Failure, Success, begin, buffer_out, call, settled, wait_pending
from ._errors import SessionBusyError

if TYPE_CHECKING:
    from _fujimod_js import JsChunkIterator as JsChunkIterator
    from _fujimod_js import JsIteratorResult as JsIteratorResult


class Reader:
    _js: JsChunkIterator
    _pending: asyncio.Task[Success[JsIteratorResult] | Failure] | None

    def __init__(self) -> None:
        raise TypeError("use memory.read_chunks() or data.read_chunks()")

    @classmethod
    def _wrap(cls, reader: JsChunkIterator) -> Self:
        obj = object.__new__(cls)
        obj._js = reader
        obj._pending = None
        return obj

    @property
    def bytes_completed(self) -> int:
        return self._js.bytesCompleted

    def __aiter__(self) -> Self:
        return self

    async def __anext__(self) -> bytes:
        if self._pending is not None:
            raise SessionBusyError("range reader already has a pending chunk")
        # The JS facade turns its Rust reader's optional chunk into IteratorResult.
        pending = begin(call(self._js.next))
        self._pending = pending
        try:
            value = await wait_pending(pending, self._js.cancelPending)
            if value.done:
                raise StopAsyncIteration
            return buffer_out(value.value)
        finally:
            self._pending = None

    async def aclose(self) -> None:
        pending = self._pending
        if pending is not None:
            call(self._js.cancelPending)
            await settled(pending)
        else:
            call(self._js.close)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> Literal[False]:
        await self.aclose()
        return False

    def __del__(self) -> None:
        value = getattr(self, "_js", None)
        if value is not None:
            # No implicit camera I/O, including when iteration ends early.
            value.cancelPending()


Reader.__module__ = "fujimod.usb"
