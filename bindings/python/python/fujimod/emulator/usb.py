"""Opaque USB bus operations and paused observations on a native endpoint."""

import json
from typing import Protocol, cast

type _JsonValue = (
    None | bool | int | float | str | list[_JsonValue] | dict[str, _JsonValue]
)


class _UsbRuntime(Protocol):
    def usb_status(self, node: str) -> str: ...

    def usb_reset(self, node: str) -> None: ...

    def usb_setup(self, node: str, packet: bytes) -> None: ...

    def usb_out(self, node: str, endpoint: int, packet: bytes) -> None: ...

    def usb_in(self, node: str, endpoint: int, size: int) -> bytes: ...

    def usb_in_token(self, node: str, endpoint: int) -> None: ...


class EndpointPort:
    def __init__(self, runtime: _UsbRuntime, node: str = "usb") -> None:
        self.runtime, self.node = runtime, node

    def status(self) -> _JsonValue:
        return cast(_JsonValue, json.loads(self.runtime.usb_status(self.node)))

    def reset(self) -> None:
        self.runtime.usb_reset(self.node)

    def setup(self, packet: bytes) -> None:
        self.runtime.usb_setup(self.node, packet)

    def out(self, endpoint: int, packet: bytes) -> None:
        self.runtime.usb_out(self.node, endpoint, packet)

    def in_packet(self, endpoint: int, size: int) -> bytes:
        return self.runtime.usb_in(self.node, endpoint, size)

    def in_token(self, endpoint: int) -> None:
        self.runtime.usb_in_token(self.node, endpoint)
