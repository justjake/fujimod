"""Pull immutable snapshots from the Rust bounded trace queue."""

from dataclasses import dataclass
from enum import Enum, auto
from typing import TYPE_CHECKING, Self

from .._bridge import Handle, buffer_out, call, enum_out, jsusb, snapshot, uint

if TYPE_CHECKING:
    from _fujimod_js import JsProtocolTraceEvent as JsProtocolTraceEvent
    from _fujimod_js import JsTraceEvent as JsTraceEvent


class Layer(Enum):
    Ff80 = auto()
    App80 = auto()


class Stage(Enum):
    Control = auto()
    BulkIn = auto()
    BulkOut = auto()


class Phase(Enum):
    Submitted = auto()
    Completed = auto()
    Failed = auto()
    Canceled = auto()


class RequestKind(Enum):
    Open = auto()
    Ping = auto()
    Close = auto()
    WriteCommand = auto()
    ReadCommand = auto()
    WriteData = auto()
    ReadData = auto()
    EmptyResponse = auto()


class FailureKind(Enum):
    Transfer = auto()
    Timeout = auto()
    ShortTransfer = auto()
    Other = auto()


@dataclass(frozen=True)
class Event:
    sequence: int
    transfer: int
    timestamp: float
    endpoint: int | None
    stage: Stage
    phase: Phase
    request_kind: RequestKind | None
    data: bytes
    requested: int | None
    transferred: int | None
    failure_kind: FailureKind | None
    error: str | None

    @classmethod
    def _from_js(cls, value: JsTraceEvent) -> Self:
        def convert(v: JsTraceEvent) -> Self:
            request = (
                enum_out(v.requestKind, RequestKind, "RequestKind")
                if v.requestKind is not None
                else None
            )
            failure = (
                {getattr(jsusb.tracing.FailureKind, m.name): m for m in FailureKind}[
                    v.failureKind
                ]
                if v.failureKind is not None
                else None
            )
            return cls(
                int(v.sequence),
                int(v.transfer),
                v.timestampMs / 1000,
                v.endpoint,
                enum_out(v.stage, Stage, "TransferStage"),
                {getattr(jsusb.tracing.Phase, m.name): m for m in Phase}[v.phase],
                request,
                buffer_out(v.data),
                v.requested,
                v.transferred,
                failure,
                v.failureMessage,
            )

        return snapshot(value, convert)


@dataclass(frozen=True)
class ProtocolEvent:
    sequence: int
    timestamp: float
    exchange: int
    layer: Layer
    command: int
    phase: Phase
    data: bytes
    error: str | None

    @classmethod
    def _from_js(cls, value: JsProtocolTraceEvent) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                int(v.sequence),
                v.timestampMs / 1000,
                int(v.exchange),
                {getattr(jsusb.tracing.Layer, m.name): m for m in Layer}[v.layer],
                v.command,
                {getattr(jsusb.tracing.Phase, m.name): m for m in Phase}[v.phase],
                buffer_out(v.data),
                v.failureMessage,
            ),
        )


@dataclass(frozen=True)
class Batch:
    events: tuple[Event, ...]
    protocol_events: tuple[ProtocolEvent, ...]
    dropped_events: int
    dropped_bytes: int


class Trace(Handle["JsTrace"]):
    def __init__(self, max_events: int, max_bytes: int) -> None:
        self._js = call(jsusb.tracing.Trace.new, uint(max_events), uint(max_bytes))

    def drain(self) -> Batch:
        return snapshot(
            call(self._js.drain),
            lambda v: Batch(
                tuple(Event._from_js(e) for e in v.events),
                tuple(ProtocolEvent._from_js(e) for e in v.protocolEvents),
                int(v.droppedEvents),
                int(v.droppedBytes),
            ),
        )


if TYPE_CHECKING:
    from _fujimod_js import JsTrace as JsTrace
