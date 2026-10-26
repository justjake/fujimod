from collections.abc import Buffer, Sequence
from typing import Protocol

from . import FirmwareLimits, Session, tracing

class Exchange:
    @staticmethod
    def control(bytes: Buffer) -> Exchange: ...
    @staticmethod
    def control_disconnected(bytes: Buffer) -> Exchange: ...
    @staticmethod
    def write(bytes: Buffer) -> Exchange: ...
    @staticmethod
    def read(maximum: int, data: Buffer) -> Exchange: ...
    @staticmethod
    def pending_read(maximum: int) -> Exchange: ...
    @staticmethod
    def delayed_read(maximum: int, data: Buffer, delay_ms: int) -> Exchange: ...
    @staticmethod
    def disconnected(maximum: int, partial: Buffer) -> Exchange: ...

class Script:
    @property
    def remaining(self) -> int: ...

class Peer(Protocol):
    def control(self, data: bytes, /) -> object: ...
    def write(self, data: bytes, /) -> object: ...
    def read(self, maximum: int) -> object: ...

async def session(
    exchanges: Sequence[Exchange],
    limits: FirmwareLimits,
    *,
    max_reply_bytes: int = ...,
    trace: tracing.Trace | None = ...,
) -> tuple[Session, Script]: ...
async def peer_session(
    peer: Peer,
    limits: FirmwareLimits,
    *,
    max_reply_bytes: int = ...,
    trace: tracing.Trace | None = ...,
    ep_in: int = ...,
    ep_out: int = ...,
) -> Session: ...
