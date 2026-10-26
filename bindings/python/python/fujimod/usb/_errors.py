"""Python exception categories shared by the browser adapter's entry points."""


class UsbError(RuntimeError):
    command: int
    primary: int
    secondary: int
    transferred: int | None
    expected: int
    actual: int
    available: int
    declared: int
    requested: int
    limit: int
    reserved: int
    partial: bytes
    transfer_error: UsbError


class TransferError(UsbError):
    pass


class ProtocolError(UsbError):
    pass


class CompletionUnknownError(UsbError):
    pass


class DeviceCommandError(UsbError):
    pass


class ClosedSessionError(UsbError):
    pass


class SessionBusyError(UsbError):
    pass


for _exception in (
    UsbError,
    TransferError,
    ProtocolError,
    CompletionUnknownError,
    DeviceCommandError,
    ClosedSessionError,
    SessionBusyError,
):
    _exception.__module__ = "fujimod.usb"
