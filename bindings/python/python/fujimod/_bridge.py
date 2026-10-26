"""Language conversions only. Every USB operation executes in the Rust module."""

import asyncio
from collections.abc import Awaitable, Buffer, Callable
from contextlib import suppress
from dataclasses import dataclass
from enum import Enum
from typing import TYPE_CHECKING, Protocol, Self, cast

from _fujimod_js import usb as jsusb
from js import Uint8Array
from pyodide.ffi import JsException, to_js

if TYPE_CHECKING:
    from _fujimod_js import JsBytes, JsDefinitionError, JsServiceCommandError


class JsFailure(Protocol):
    js_error: object


class JsFree(Protocol):
    def free(self) -> None: ...


def uint(value: int, bits: int = 32) -> int:
    if type(value) is not int:
        raise TypeError("expected an integer quantity")
    if not 0 <= value < (1 << bits):
        raise OverflowError(f"quantity must fit u{bits}")
    return value


def sint(value: int, bits: int) -> int:
    if type(value) is not int:
        raise TypeError("expected an integer quantity")
    if not -(1 << (bits - 1)) <= value < (1 << (bits - 1)):
        raise OverflowError(f"quantity must fit i{bits}")
    return value


def buffer_in(value: Buffer) -> JsBytes:
    # Copy before yielding: JS/Rust must never retain a Python-memory view.
    return Uint8Array.new(to_js(memoryview(value).cast("B"), create_pyproxies=False))


def buffer_out(value: JsBytes) -> bytes:
    return bytes(value.to_py())


def enum_in[E: Enum](value: E, cls: type[E], group: str | None = None) -> int:
    if type(value) is not cls:
        raise TypeError(f"expected {cls.__name__}")
    return cast(int, getattr(getattr(jsusb, group or cls.__name__), value.name))


def enum_out[E: Enum](value: int, cls: type[E], group: str | None = None) -> E:
    names = getattr(jsusb, group or cls.__name__)
    return {getattr(names, member.name): member for member in cls}[value]


def convert_error(value: object) -> Exception:
    kind = getattr(value, "kind", None)
    kinds = jsusb.ErrorKind
    if kind == kinds.Definition:
        from .catalog import DefinitionError, _failure

        error = DefinitionError(str(getattr(value, "message", value)))
        error.reason = _failure(cast("JsDefinitionError", value).reason)
        return error
    if kind == kinds.ServiceCommand:
        from .usb._catalog import CommandError, _command_failure

        error = CommandError(str(getattr(value, "message", value)))
        error.reason = _command_failure(cast("JsServiceCommandError", value).reason)
        return error
    from .usb._errors import (
        ClosedSessionError,
        CompletionUnknownError,
        DeviceCommandError,
        ProtocolError,
        SessionBusyError,
        TransferError,
        UsbError,
    )

    cls = {
        kinds.Transfer: TransferError,
        kinds.Protocol: ProtocolError,
        kinds.DeviceStatus: DeviceCommandError,
        kinds.CompletionUnknown: CompletionUnknownError,
        kinds.InvalidArgument: ValueError,
        kinds.Busy: SessionBusyError,
        kinds.Closed: ClosedSessionError,
        kinds.Abandoned: ClosedSessionError,
        kinds.Canceled: ClosedSessionError,
    }.get(kind, UsbError)
    error = cls(str(getattr(value, "message", value)))
    fields: dict[str, object] = {}
    for name in (
        "command",
        "primary",
        "secondary",
        "transferred",
        "expected",
        "actual",
        "available",
        "declared",
        "requested",
        "limit",
        "reserved",
    ):
        field = getattr(value, name, None)
        if field is not None:
            fields[name] = field
    partial = getattr(value, "partial", None)
    if partial is not None:
        fields["partial"] = buffer_out(partial)
    if cls is TransferError:
        fields.setdefault("partial", b"")
        fields.setdefault("transferred", None)
    cause = getattr(value, "cause", None)
    if cause is not None:
        fields["transfer_error"] = convert_error(cause)
    for name, field in fields.items():
        setattr(error, name, field)
    return error


def call[**P, T](function: Callable[P, T], *args: P.args, **kwargs: P.kwargs) -> T:
    try:
        return function(*args, **kwargs)
    except JsException as error:
        raise convert_error(cast(JsFailure, error).js_error) from error


async def settled[T](future: asyncio.Future[T]) -> None:
    """Finish cancellation cleanup even when the caller is canceled again."""
    while not future.done():
        try:
            await asyncio.shield(future)
        except asyncio.CancelledError:
            continue
        except Exception:
            break
    # Retrieve the outcome so a rejected JS Promise never becomes unhandled.
    if not future.cancelled():
        with suppress(Exception):
            future.result()


@dataclass(frozen=True)
class Success[T]:
    value: T


@dataclass(frozen=True)
class Failure:
    error: Exception


def begin[T](promise: Awaitable[T]) -> asyncio.Task[Success[T] | Failure]:
    async def capture() -> Success[T] | Failure:
        try:
            return Success(await promise)
        except Exception as error:
            return Failure(error)

    # A shielded inner task always completes with an explicit outcome, avoiding
    # asyncio's exception-on-cancel logging while we consume cancellation.
    return asyncio.create_task(capture())


async def wait_pending[T](
    future: asyncio.Future[Success[T] | Failure],
    cancel: Callable[[], object] | None = None,
) -> T:
    try:
        outcome = await asyncio.shield(future)
    except asyncio.CancelledError:
        if cancel is not None:
            cancel()
        await settled(future)
        raise
    if isinstance(outcome, Failure):
        if isinstance(outcome.error, JsException):
            raise convert_error(
                cast(JsFailure, outcome.error).js_error
            ) from outcome.error
        raise outcome.error
    return outcome.value


async def await_result[T](
    promise: Awaitable[T], cancel: Callable[[], object] | None = None
) -> T:
    return await wait_pending(begin(promise), cancel)


class Handle[T]:
    """An owned wasm-bindgen object. Destruction releases memory, never USB I/O."""

    _js: T

    @classmethod
    def _wrap(cls, value: T) -> Self:
        obj = object.__new__(cls)
        object.__setattr__(obj, "_js", value)
        return obj

    def __del__(self) -> None:
        value = getattr(self, "_js", None)
        if value is not None:
            value.free()


def snapshot[U: JsFree, T](value: U, convert: Callable[[U], T]) -> T:
    try:
        return convert(value)
    finally:
        value.free()


def unwrap[T](value: object, cls: type[Handle[T]]) -> T:
    if type(value) is not cls:
        raise TypeError(f"expected {cls.__name__}")
    return cast(Handle[T], value)._js
