"""Pyodide conversion for Rust-owned service commands and typed replies."""

from collections.abc import Buffer
from dataclasses import dataclass
from types import SimpleNamespace
from typing import TYPE_CHECKING, Self, cast

from _fujimod_js import valueSchemas as _schemas

from .._binding_values import bind_values as _bind_values
from .._bridge import Handle, buffer_in, buffer_out, call, jsusb, snapshot, unwrap
from .._variants import Sum, enum_out, unwrap_variant, variant
from ..catalog import service as definitions
from . import Session
from .app80 import CommandCode

if TYPE_CHECKING:
    from _fujimod_js import (
        JsApp80CommandReply,
        JsApp80Invocation,
        JsApp80Service,
        JsCommandFailure,
    )


_js = jsusb.app80
_value_types, _result_decoders = _bind_values(
    _schemas.service, _js, "fujimod.usb.app80"
)
if TYPE_CHECKING:
    import _fujimod_service_values as _v
else:
    _v = SimpleNamespace(**_value_types)


class CommandError(ValueError):
    reason: CommandFailure


if TYPE_CHECKING:
    from .._command_types import CommandFailure as CommandFailure
else:

    class CommandFailure(Sum):
        pass


variant(CommandFailure, "Unsupported", [("codec", definitions.Codec)])
variant(CommandFailure, "Ambiguous", [("codec", definitions.Codec)])
variant(CommandFailure, "Unavailable", [("code", int)])
variant(CommandFailure, "Argument", [("kind", _v.ArgumentKind)])
variant(CommandFailure, "OversizeArguments")


def _command_failure(value: JsCommandFailure) -> CommandFailure:
    from _fujimod_js import catalog

    kinds = _js.CommandFailureKind
    if value.kind == kinds.Unsupported:
        return CommandFailure.Unsupported(
            enum_out(value.codec, definitions.Codec, catalog.service.Codec)
        )
    if value.kind == kinds.Ambiguous:
        return CommandFailure.Ambiguous(
            enum_out(value.codec, definitions.Codec, catalog.service.Codec)
        )
    if value.kind == kinds.Unavailable:
        return CommandFailure.Unavailable(value.code)
    if value.kind == kinds.Argument:
        return CommandFailure.Argument(
            enum_out(value.argument, _v.ArgumentKind, _js.ArgumentKind)
        )
    return CommandFailure.OversizeArguments()


@dataclass(frozen=True)
class CommandReply:
    value: _v.ReplyValue
    raw: bytes

    @classmethod
    def _from_js(cls, value: JsApp80CommandReply) -> Self:
        return snapshot(
            value,
            lambda reply: cls(
                cast("_v.ReplyValue", _result_decoders[_v.ReplyValue](reply.value)),
                buffer_out(reply.raw),
            ),
        )


class Invocation(Handle["JsApp80Invocation"]):
    def __init__(self) -> None:
        raise TypeError("invocations come from Service.prepare")

    @property
    def raw(self) -> bytes:
        return buffer_out(self._js.raw)

    @property
    def configuration_limit(self) -> int:
        return self._js.configurationLimit

    def decode(self, raw: Buffer) -> CommandReply:
        return CommandReply._from_js(call(self._js.decode, buffer_in(raw)))


class Service(Handle["JsApp80Service"]):
    def __init__(self, definition: definitions.Definition) -> None:
        self._js = call(_js.Service.new, unwrap(definition, definitions.Definition))

    def prepare(
        self, modes: definitions.ServiceModes, command: _v.Command
    ) -> Invocation:
        return Invocation._wrap(
            call(
                self._js.prepare,
                unwrap(modes, definitions.ServiceModes),
                unwrap_variant(command, _v.Command),
            )
        )

    def prepare_code(
        self, code: CommandCode, modes: definitions.ServiceModes, command: _v.Command
    ) -> Invocation:
        return Invocation._wrap(
            call(
                self._js.prepareCode,
                unwrap(code, CommandCode),
                unwrap(modes, definitions.ServiceModes),
                unwrap_variant(command, _v.Command),
            )
        )

    def prepare_research(
        self, code: CommandCode, modes: definitions.ServiceModes, arguments: Buffer
    ) -> Invocation:
        return Invocation._wrap(
            call(
                self._js.prepareResearch,
                unwrap(code, CommandCode),
                unwrap(modes, definitions.ServiceModes),
                buffer_in(arguments),
            )
        )


async def execute(session: Session, invocation: Invocation) -> CommandReply:
    return await session._call(
        _js.execute,
        CommandReply._from_js,
        unwrap(session, Session),
        unwrap(invocation, Invocation),
    )


if TYPE_CHECKING:
    from _fujimod_js import JsApp80Invocation as JsApp80Invocation
    from _fujimod_js import JsApp80Service as JsApp80Service
