"""Awaitable Pyodide API over the same Rust core as native FujiMod."""

from collections.abc import Awaitable, Buffer, Callable, Sequence
from typing import TYPE_CHECKING, Self

from .._bridge import (
    Handle,
    await_result,
    buffer_in,
    buffer_out,
    call,
    enum_in,
    enum_out,
    jsusb,
    snapshot,
    uint,
    unwrap,
)
from ._errors import (
    ClosedSessionError,
    CompletionUnknownError,
    DeviceCommandError,
    ProtocolError,
    SessionBusyError,
    TransferError,
    UsbError,
)
from ._reader import Reader
from ._types import (
    CameraMode,
    CapabilitiesReply,
    CommandStatus,
    ConfigurationGeometry,
    ConfigurationView,
    ConsoleProgress,
    ConsoleReply,
    DataReadMode,
    DataWindow,
    DataWriteMode,
    Device,
    DeviceFilter,
    DiagnosticReply,
    DiagnosticTag,
    FirmwareDownloadReply,
    FirmwareDownloadState,
    FirmwareLimits,
    InfoSelector,
    InterfaceSelection,
    KeyState,
    LogReply,
    OffsetMode,
    SessionState,
)

if TYPE_CHECKING:
    from _fujimod_js import (
        JsCommandSequence as JsCommandSequence,
    )
    from _fujimod_js import (
        JsInstallQueries as JsInstallQueries,
    )
    from _fujimod_js import (
        JsOwnedUpload as JsOwnedUpload,
    )
    from _fujimod_js import (
        JsSession as JsSession,
    )

    from .data import UploadReply


class CommandSequence(Handle["JsCommandSequence"]):
    def __init__(self) -> None:
        self._js = call(jsusb.CommandSequence.new)

    def read(self, address: int, length: int, expected: Buffer | None = None) -> int:
        return call(
            self._js.read,
            uint(address),
            uint(length),
            None if expected is None else buffer_in(expected),
        )

    def write(self, address: int, data: Buffer) -> int:
        return call(self._js.write, uint(address), buffer_in(data))

    def query(self, tag: int, statuses: Sequence[tuple[int, int]] | None = None) -> int:
        from pyodide.ffi import to_js

        pairs = [
            (uint(primary), uint(secondary))
            for primary, secondary in ([(0, 0)] if statuses is None else statuses)
        ]
        return call(self._js.query, uint(tag), to_js(pairs, create_pyproxies=False))

    @classmethod
    def verified_writes(
        cls, address: int, data: Buffer, limits: FirmwareLimits, packet_bytes: int
    ) -> list[Self]:
        return [
            cls._wrap(batch)
            for batch in call(
                jsusb.CommandSequence.verifiedWrites,
                uint(address),
                buffer_in(data),
                unwrap(limits, FirmwareLimits),
                uint(packet_bytes),
            )
        ]


class InstallQueries(Handle["JsInstallQueries"]):
    def __init__(
        self, callback: int, stock: int, output: int, original: Buffer
    ) -> None:
        self._js = call(
            jsusb.InstallQueries.new,
            uint(callback),
            uint(stock),
            uint(output),
            buffer_in(original),
        )

    def call(
        self,
        callback: int,
        tag: int,
        statuses: Sequence[tuple[int, int]],
        output: Buffer,
    ) -> tuple[int, int]:
        from pyodide.ffi import to_js

        pairs = [(uint(primary), uint(secondary)) for primary, secondary in statuses]
        values = call(
            self._js.call,
            uint(callback),
            uint(tag),
            to_js(pairs, create_pyproxies=False),
            buffer_in(output),
        ).to_py()
        first, second = values
        return first, second

    def finish(self) -> CommandSequence:
        return CommandSequence._wrap(call(self._js.finish))


class OwnedUpload(Handle["JsOwnedUpload"]):
    def __init__(
        self,
        allocation_address: int,
        allocation_size: int,
        address: int,
        data: Buffer,
        piece_bytes: int,
        alignment: int,
    ) -> None:
        self._js = call(
            jsusb.OwnedUpload.new,
            uint(allocation_address),
            uint(allocation_size),
            uint(address),
            buffer_in(data),
            uint(piece_bytes),
            uint(alignment),
        )

    @staticmethod
    def padded_size(length: int, piece_bytes: int) -> int:
        return call(jsusb.OwnedUpload.paddedSize, uint(length), uint(piece_bytes))

    image_bytes = property(lambda self: self._js.imageBytes)
    padded_bytes = property(lambda self: self._js.paddedBytes)


class Session(Handle["JsSession"]):
    def __init__(self) -> None:
        raise TypeError("use await Session.open(...)")

    @classmethod
    async def open(
        cls,
        device: Device,
        interface: InterfaceSelection,
        limits: FirmwareLimits,
        *,
        timeout_ms: int = 5000,
        max_reply_bytes: int = 8388608,
        trace: tracing.Trace | None = None,
    ) -> Self:
        options = call(
            jsusb.TransferOptions.new, uint(timeout_ms), uint(max_reply_bytes)
        )
        try:
            if trace is not None:
                call(options.setTrace, unwrap(trace, tracing.Trace))
            promise = call(
                jsusb.Session.open,
                unwrap(device, Device),
                unwrap(interface, InterfaceSelection),
                unwrap(limits, FirmwareLimits),
                options,
            )
            # The owner exists only once C0 completes. Consume a canceled
            # open's outcome and abandon it without any cleanup request.
            import asyncio

            from .._bridge import Success, begin, wait_pending

            pending = begin(promise)
            try:
                return cls._wrap(await wait_pending(pending))
            except asyncio.CancelledError:
                outcome = pending.result()
                if isinstance(outcome, Success):
                    outcome.value.abandon()
                    outcome.value.free()
                raise
        finally:
            options.free()

    @property
    def state(self) -> SessionState:
        return enum_out(self._js.state, SessionState)

    @property
    def token(self) -> int:
        return self._js.sessionId

    @property
    def packet_bytes(self) -> int:
        return self._js.packetBytes

    async def _call[**P, U, T](
        self,
        method: Callable[P, Awaitable[U]],
        decode: Callable[[U], T],
        /,
        *args: P.args,
        **kwargs: P.kwargs,
    ) -> T:
        return decode(
            await await_result(call(method, *args, **kwargs), self._js.cancelPending)
        )

    async def ping(self) -> bytes:
        return await self._call(self._js.ping, buffer_out)

    async def execute_sequence(self, sequence: CommandSequence) -> list[bytes]:
        return await self._call(
            self._js.executeSequence,
            lambda replies: [buffer_out(r) for r in replies],
            unwrap(sequence, CommandSequence),
        )

    async def upload_owned(
        self,
        image: OwnedUpload,
        pointer: int,
        original: int,
        capacity_address: int,
        capacity: int,
    ) -> UploadReply:
        from .data import UploadReply

        return await self._call(
            self._js.uploadOwned,
            UploadReply._from_js,
            unwrap(image, OwnedUpload),
            uint(pointer),
            uint(original),
            uint(capacity_address),
            uint(capacity),
        )

    async def verify_owned(self, image: OwnedUpload, window: DataWindow) -> None:
        await self._call(
            self._js.verifyOwned,
            lambda value: value,
            unwrap(image, OwnedUpload),
            unwrap(window, DataWindow),
        )

    async def close(self) -> bytes:
        return await self._call(self._js.close, buffer_out)

    def abandon(self) -> None:
        if self.state in (SessionState.Closed, SessionState.Abandoned):
            raise ClosedSessionError("session is no longer available")
        call(self._js.abandon)

    async def device_info(self, selector: InfoSelector) -> str:
        return await self._call(
            self._js.deviceInfo,
            _info_text,
            enum_in(selector, InfoSelector),
        )

    async def read_memory(self, address: int, length: int) -> bytes:
        return await self._call(
            self._js.readMemory, buffer_out, uint(address), uint(length)
        )

    async def write_memory(self, address: int, data: Buffer) -> None:
        await self._call(
            self._js.writeMemory, (lambda value: value), uint(address), buffer_in(data)
        )

    async def configuration_geometry(
        self, view: ConfigurationView
    ) -> ConfigurationGeometry:
        return await self._call(
            self._js.configurationGeometry,
            ConfigurationGeometry._from_js,
            enum_in(view, ConfigurationView),
        )

    async def read_configuration(
        self, view: ConfigurationView, offset: int, length: int, *, page: int = 0
    ) -> bytes:
        return await self._call(
            self._js.readConfiguration,
            buffer_out,
            enum_in(view, ConfigurationView),
            uint(page),
            uint(offset),
            uint(length),
        )

    async def write_configuration(
        self, view: ConfigurationView, offset: int, data: Buffer, *, page: int = 0
    ) -> None:
        await self._call(
            self._js.writeConfiguration,
            (lambda value: value),
            enum_in(view, ConfigurationView),
            uint(page),
            uint(offset),
            buffer_in(data),
        )

    async def diagnostic_query(self, tag: DiagnosticTag) -> DiagnosticReply:
        return await self._call(
            self._js.diagnosticQuery,
            DiagnosticReply._from_js,
            unwrap(tag, DiagnosticTag),
        )

    async def prepare_data_read(self, mode: DataReadMode) -> DiagnosticReply:
        return await self._call(
            self._js.prepareDataRead,
            DiagnosticReply._from_js,
            enum_in(mode, DataReadMode),
        )

    async def prepare_data_write(
        self, length: int, mode: DataWriteMode
    ) -> DiagnosticReply:
        return await self._call(
            self._js.prepareDataWrite,
            DiagnosticReply._from_js,
            uint(length),
            enum_in(mode, DataWriteMode),
        )

    async def read_data(self, offset: int, length: int) -> bytes:
        return await self._call(
            self._js.readData, buffer_out, uint(offset), uint(length)
        )

    async def write_data(self, data: Buffer) -> None:
        await self._call(self._js.writeData, (lambda value: value), buffer_in(data))

    async def empty_response(self) -> bytes:
        return await self._call(self._js.emptyResponse, buffer_out)

    async def prepare_capabilities(self) -> CapabilitiesReply:
        return await self._call(
            self._js.prepareCapabilities, CapabilitiesReply._from_js
        )

    async def firmware_download_status(self) -> FirmwareDownloadReply:
        return await self._call(
            self._js.firmwareDownloadStatus, FirmwareDownloadReply._from_js
        )

    async def clear_firmware_download_status(self) -> FirmwareDownloadReply:
        return await self._call(
            self._js.clearFirmwareDownloadStatus, FirmwareDownloadReply._from_js
        )

    async def send_key(self, name: str, state: KeyState) -> bytes:
        if not isinstance(name, str):
            raise TypeError("expected event name text")
        return await self._call(
            self._js.sendKey, buffer_out, name, enum_in(state, KeyState)
        )

    async def request_mode(self, mode: CameraMode) -> bytes:
        return await self._call(
            self._js.requestMode, buffer_out, enum_in(mode, CameraMode)
        )

    async def command_event(self, argument: str) -> bytes:
        if not isinstance(argument, str):
            raise TypeError("expected command event text")
        return await self._call(self._js.commandEvent, buffer_out, argument)

    async def submit_console(self, data: Buffer) -> None:
        await self._call(self._js.submitConsole, (lambda value: value), buffer_in(data))

    async def read_log(self, capacity: int) -> LogReply:
        return await self._call(self._js.readLog, LogReply._from_js, uint(capacity))

    async def poll_console(self, capacity: int) -> ConsoleReply:
        return await self._call(
            self._js.pollConsole, ConsoleReply._from_js, uint(capacity)
        )

    async def research_command(self, request: research.Request) -> research.Reply:
        return await self._call(
            self._js.researchCommand,
            research.Reply._from_js,
            unwrap(request, research.Request),
        )


async def discover(filter: DeviceFilter) -> list[Device]:
    return [
        Device._wrap(v)
        for v in await await_result(call(jsusb.discover, enum_in(filter, DeviceFilter)))
    ]


async def discover_proxy(url: str, token: str, filter: DeviceFilter) -> list[Device]:
    """Discover remote devices through a relay, including in Pyodide."""
    if not isinstance(url, str) or not isinstance(token, str):
        raise TypeError("relay URL and token must be strings")
    return [
        Device._wrap(v)
        for v in await await_result(
            call(jsusb.discoverProxy, url, token, enum_in(filter, DeviceFilter))
        )
    ]


async def request_device(filter: DeviceFilter) -> Device | None:
    """Browser picker: invoke directly from a user gesture."""
    value = await await_result(call(jsusb.requestDevice, enum_in(filter, DeviceFilter)))
    return None if value is None else Device._wrap(value)


async def interfaces(device: Device) -> list[InterfaceSelection]:
    return [
        InterfaceSelection._from_js(v)
        for v in await await_result(call(unwrap(device, Device).interfaces))
    ]


from . import app80, data, memory, research, tracing  # noqa: E402

__all__ = [
    "Session",
    "Reader",
    "discover",
    "discover_proxy",
    "request_device",
    "interfaces",
    "memory",
    "data",
    "app80",
    "research",
    "tracing",
    "UsbError",
    "TransferError",
    "ProtocolError",
    "CompletionUnknownError",
    "DeviceCommandError",
    "ClosedSessionError",
    "SessionBusyError",
    "InfoSelector",
    "ConfigurationView",
    "KeyState",
    "CameraMode",
    "DataReadMode",
    "DataWriteMode",
    "OffsetMode",
    "DeviceFilter",
    "SessionState",
    "CommandStatus",
    "ConsoleProgress",
    "FirmwareDownloadState",
    "FirmwareLimits",
    "InterfaceSelection",
    "Device",
    "DiagnosticTag",
    "DataWindow",
    "DiagnosticReply",
    "FirmwareDownloadReply",
    "ConfigurationGeometry",
    "CapabilitiesReply",
    "LogReply",
    "ConsoleReply",
]


def _info_text(reply: JsInfoReply) -> str:
    return snapshot(reply, lambda value: value.text)


if TYPE_CHECKING:
    from _fujimod_js import JsInfoReply as JsInfoReply
