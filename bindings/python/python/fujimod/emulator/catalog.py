"""Typed identity-based access to the bundled hardware reference catalog."""

from __future__ import annotations

import json
import re
from collections.abc import Sequence
from typing import Protocol, TypeGuard, cast

from fujimod._native import catalog as _native_catalog_module

from ._catalog_types import (
    Body,
    Firmware,
    Part,
    ResolvedConfiguration,
    ResolvedSelection,
    RunConfiguration,
    RunOverlay,
    Soc,
)


class _NativeCatalogResolver(Protocol):
    def resolve_reference(self, body_id: str, firmware_id: str) -> str: ...

    def resolve_firmware(self, firmware_id: str) -> str: ...

    def resolve_configuration(
        self, configuration_id: str, overlays: list[str]
    ) -> str: ...


_native_catalog: _NativeCatalogResolver = cast(
    _NativeCatalogResolver, _native_catalog_module
)


def _string_keyed_object(value: object) -> TypeGuard[dict[str, object]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in value)


def _document(result: object) -> dict[str, object]:
    if type(result) is not str:
        raise RuntimeError("Native catalog resolution must return JSON text")
    value = json.loads(result)
    if not _string_keyed_object(value):
        raise RuntimeError("Native catalog resolution must return a JSON object")
    return value


def _selection(result: object) -> ResolvedSelection:
    return cast(ResolvedSelection, _document(result))


def resolve_reference(body_id: str, firmware_id: str) -> ResolvedSelection:
    """Resolve a checked body and firmware selection by their public IDs."""
    if type(body_id) is not str or type(firmware_id) is not str:
        raise TypeError("body_id and firmware_id must be strings")
    return _selection(_native_catalog.resolve_reference(body_id, firmware_id))


def resolve_firmware(firmware_id: str) -> ResolvedSelection:
    """Resolve a firmware ID and its referenced body, SoC and parts."""
    if type(firmware_id) is not str:
        raise TypeError("firmware_id must be a string")
    return _selection(_native_catalog.resolve_firmware(firmware_id))


def resolve_configuration(
    configuration_id: str, *, overlays: Sequence[str] = ()
) -> ResolvedConfiguration:
    """Resolve a run configuration, selected overlays and checked references."""
    if type(configuration_id) is not str:
        raise TypeError("configuration_id must be a string")
    if isinstance(overlays, (str, bytes)) or not isinstance(overlays, Sequence):
        raise TypeError("overlays must be a sequence of catalog overlay IDs")
    selected_overlays = list(overlays)
    if any(
        type(overlay) is not str
        or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", overlay)
        for overlay in selected_overlays
    ):
        raise ValueError("overlays must contain stable lowercase catalog IDs")
    if len(selected_overlays) != len(set(selected_overlays)):
        raise ValueError("overlays must contain unique catalog identities")
    value = _native_catalog.resolve_configuration(configuration_id, selected_overlays)
    return cast(ResolvedConfiguration, _document(value))


__all__ = [
    "Body",
    "Firmware",
    "Part",
    "ResolvedConfiguration",
    "ResolvedSelection",
    "RunConfiguration",
    "RunOverlay",
    "Soc",
    "resolve_firmware",
    "resolve_configuration",
    "resolve_reference",
]
