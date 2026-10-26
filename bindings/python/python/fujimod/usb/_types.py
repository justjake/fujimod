"""Typed Python values. Wire encoding and decoding remain in Rust."""

from dataclasses import dataclass
from enum import Enum, auto
from typing import TYPE_CHECKING, Self

from .._bridge import Handle, buffer_out, call, enum_in, enum_out, jsusb, snapshot, uint

if TYPE_CHECKING:
    from _fujimod_js import (
        JsCapabilitiesReply as JsCapabilitiesReply,
    )
    from _fujimod_js import (
        JsConfigurationGeometry as JsConfigurationGeometry,
    )
    from _fujimod_js import (
        JsConsoleReply as JsConsoleReply,
    )
    from _fujimod_js import (
        JsDiagnosticReply as JsDiagnosticReply,
    )
    from _fujimod_js import (
        JsFirmwareDownloadReply as JsFirmwareDownloadReply,
    )
    from _fujimod_js import (
        JsFirmwareLimits as JsFirmwareLimits,
    )
    from _fujimod_js import (
        JsInterfaceSelection as JsInterfaceSelection,
    )
    from _fujimod_js import (
        JsLogReply as JsLogReply,
    )


class InfoSelector(Enum):
    Vendor = auto()
    Manufacturer = auto()
    Product = auto()
    InternalVersion = auto()
    DeviceType = auto()
    Serial = auto()
    Framework = auto()


class ConfigurationView(Enum):
    Storage = auto()
    Ram = auto()


class KeyState(Enum):
    On = auto()
    Off = auto()


class CameraMode(Enum):
    Record = auto()
    Playback = auto()


class DataReadMode(Enum):
    Pattern = auto()
    Echo = auto()


class DataWriteMode(Enum):
    Preserve = auto()
    FillOnes = auto()
    Echo = auto()


class OffsetMode(Enum):
    Checked = auto()
    Wrapping = auto()


class DeviceFilter(Enum):
    Fujifilm = auto()
    Diagnostic = auto()


class SessionState(Enum):
    Ready = auto()
    InFlight = auto()
    CompletionUnknown = auto()
    Closed = auto()
    Abandoned = auto()
    ReadingRange = auto()


class CommandStatus(Enum):
    Success = auto()
    DeviceError = auto()


class ConsoleProgress(Enum):
    Complete = auto()
    Pending = auto()
    Unknown = auto()


class FirmwareDownloadState(Enum):
    Normal = auto()
    Writing = auto()
    Success = auto()
    Failed = auto()
    Unknown = auto()


@dataclass(frozen=True, kw_only=True)
class FirmwareLimits(Handle["JsFirmwareLimits"]):
    memory_read: int
    memory_write: int
    configuration: int
    console: int

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "_js",
            call(
                jsusb.FirmwareLimits.new,
                *(
                    uint(n)
                    for n in (
                        self.memory_read,
                        self.memory_write,
                        self.configuration,
                        self.console,
                    )
                ),
            ),
        )


@dataclass(frozen=True, kw_only=True)
class InterfaceSelection(Handle["JsInterfaceSelection"]):
    configuration: int
    interface: int
    alternate: int

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "_js",
            call(
                jsusb.InterfaceSelection.new,
                *(
                    uint(n, 8)
                    for n in (self.configuration, self.interface, self.alternate)
                ),
            ),
        )

    @classmethod
    def _from_js(cls, item: JsInterfaceSelection) -> Self:
        return snapshot(
            item,
            lambda v: cls(
                configuration=v.configuration,
                interface=v.interface,
                alternate=v.alternate,
            ),
        )


class Device(Handle["JsDevice"]):
    def __init__(self) -> None:
        raise TypeError("use discover() or request_device()")

    @property
    def vendor_id(self) -> int:
        return self._js.vendorId

    @property
    def product_id(self) -> int:
        return self._js.productId

    # WebUSB does not expose OS bus/address/port identifiers.
    @property
    def bus_id(self) -> str | None:
        return None

    @property
    def address(self) -> int | None:
        return None

    @property
    def ports(self) -> list[int] | None:
        return None

    @property
    def manufacturer(self) -> str | None:
        return self._js.manufacturer

    @property
    def product(self) -> str | None:
        return self._js.productName

    @property
    def serial(self) -> str | None:
        return self._js.serialNumber


class DiagnosticTag(Handle["JsDiagnosticTag"]):
    def __init__(self) -> None:
        raise TypeError("use bulk_source() or research(value)")

    @classmethod
    def bulk_source(cls) -> Self:
        return cls._wrap(call(jsusb.DiagnosticTag.bulkSource))

    @classmethod
    def research(cls, value: int) -> Self:
        return cls._wrap(call(jsusb.DiagnosticTag.research, uint(value)))

    @property
    def value(self) -> int:
        return self._js.value


class DataWindow(Handle["JsDataWindow"]):
    def __init__(
        self,
        base: int,
        address: int,
        length: int,
        capacity: int,
        offset_mode: OffsetMode,
    ) -> None:
        self._js = call(
            jsusb.DataWindow.new,
            uint(base),
            uint(address),
            uint(length),
            uint(capacity),
            enum_in(offset_mode, OffsetMode),
        )

    @property
    def capacity(self) -> int:
        return self._js.capacity


@dataclass(frozen=True)
class DiagnosticReply:
    status: CommandStatus
    primary: int
    secondary: int
    _fields: tuple[int, ...]
    raw: bytes

    @property
    def fields(self) -> list[int]:
        return list(self._fields)

    @classmethod
    def _from_js(cls, value: JsDiagnosticReply) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                enum_out(v.status, CommandStatus),
                v.primary,
                v.secondary,
                tuple(v.fields.to_py()),
                buffer_out(v.raw),
            ),
        )


@dataclass(frozen=True)
class FirmwareDownloadReply:
    state: FirmwareDownloadState
    failure_code: int | None
    text: str
    raw: bytes

    @classmethod
    def _from_js(cls, value: JsFirmwareDownloadReply) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                enum_out(v.state, FirmwareDownloadState),
                v.failureCode,
                v.text,
                buffer_out(v.raw),
            ),
        )


@dataclass(frozen=True)
class ConfigurationGeometry:
    capacity: int
    page_bytes: int
    raw: bytes

    @classmethod
    def _from_js(cls, value: JsConfigurationGeometry) -> Self:
        return snapshot(
            value, lambda v: cls(v.capacity, v.pageBytes, buffer_out(v.raw))
        )


@dataclass(frozen=True)
class CapabilitiesReply:
    byte_length: int
    raw: bytes

    @classmethod
    def _from_js(cls, value: JsCapabilitiesReply) -> Self:
        return snapshot(value, lambda v: cls(v.byteLength, buffer_out(v.raw)))


@dataclass(frozen=True)
class LogReply:
    status: int
    data: bytes

    @classmethod
    def _from_js(cls, value: JsLogReply) -> Self:
        return snapshot(value, lambda v: cls(v.status, buffer_out(v.data)))


@dataclass(frozen=True)
class ConsoleReply:
    progress: ConsoleProgress
    unknown_progress: int | None
    status: int
    output: bytes

    @classmethod
    def _from_js(cls, value: JsConsoleReply) -> Self:
        return snapshot(
            value,
            lambda v: cls(
                enum_out(v.progress, ConsoleProgress),
                v.unknownProgress,
                v.status,
                buffer_out(v.output),
            ),
        )


# Public values use the same qualified names as the native binding.
for _value in (
    InfoSelector,
    ConfigurationView,
    KeyState,
    CameraMode,
    DataReadMode,
    DataWriteMode,
    OffsetMode,
    DeviceFilter,
    SessionState,
    CommandStatus,
    ConsoleProgress,
    FirmwareDownloadState,
    FirmwareLimits,
    InterfaceSelection,
    Device,
    DiagnosticTag,
    DataWindow,
    DiagnosticReply,
    FirmwareDownloadReply,
    ConfigurationGeometry,
    CapabilitiesReply,
    LogReply,
    ConsoleReply,
):
    _value.__module__ = "fujimod.usb"


if TYPE_CHECKING:
    from _fujimod_js import JsDataWindow as JsDataWindow
    from _fujimod_js import JsDevice as JsDevice
    from _fujimod_js import JsDiagnosticTag as JsDiagnosticTag
