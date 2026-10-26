"""Explicit research envelopes, encoded and checked by Rust."""

from collections.abc import Buffer
from dataclasses import dataclass
from typing import TYPE_CHECKING, Self

from .._bridge import (
    Handle,
    buffer_in,
    buffer_out,
    call,
    enum_out,
    jsusb,
    snapshot,
    uint,
    unwrap,
)
from . import CommandStatus

if TYPE_CHECKING:
    from _fujimod_js import JsResearchReply as JsResearchReply


class CommandCode(Handle["JsResearchCommandCode"]):
    def __init__(self, value: int) -> None:
        self._js = call(jsusb.research.CommandCode.new, uint(value))

    @property
    def value(self) -> int:
        return self._js.value

    def __eq__(self, other: object) -> bool:
        return type(other) is type(self) and self.value == other.value

    def __hash__(self) -> int:
        return hash((type(self), self.value))


class Request(Handle["JsResearchRequest"]):
    def __init__(self, code: CommandCode, body: Buffer, *, reply_capacity: int) -> None:
        self._js = call(
            jsusb.research.Request.new,
            unwrap(code, CommandCode),
            buffer_in(body),
            uint(reply_capacity),
        )

    @property
    def code(self) -> CommandCode:
        return CommandCode._wrap(self._js.code)

    @property
    def body(self) -> bytes:
        return buffer_out(self._js.body)

    @property
    def reply_capacity(self) -> int:
        return self._js.replyCapacity


@dataclass(frozen=True)
class Reply:
    code: CommandCode
    status: CommandStatus
    primary: int
    secondary: int
    body: bytes
    raw: bytes

    @classmethod
    def _from_js(cls, value: JsResearchReply) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                CommandCode._wrap(v.code),
                enum_out(v.status, CommandStatus),
                v.primary,
                v.secondary,
                buffer_out(v.body),
                buffer_out(v.raw),
            ),
        )


if TYPE_CHECKING:
    from _fujimod_js import JsResearchCommandCode as JsResearchCommandCode
    from _fujimod_js import JsResearchRequest as JsResearchRequest
