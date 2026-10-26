"""Shared firmware data loaded by Rust through the JavaScript module in Pyodide."""

from typing import TYPE_CHECKING

from _fujimod_js import catalog as _jscatalog

from .._bridge import Handle, call
from .._variants import Sum, variant

if TYPE_CHECKING:
    from _fujimod_js import JsDefinitionFailure, JsFirmwareIdentity


class DefinitionError(ValueError):
    reason: DefinitionFailure


if TYPE_CHECKING:
    from .._definition_types import DefinitionFailure as DefinitionFailure
else:

    class DefinitionFailure(Sum):
        pass


variant(DefinitionFailure, "Json", [("line", int), ("column", int)])
variant(DefinitionFailure, "Schema", [("schema", int)])
for _name in (
    "EmptyIdentity",
    "InvalidDigest",
    "FirmwareMismatch",
    "EmptyModes",
    "InvalidMailbox",
    "SourceCount",
    "EmptyLabel",
):
    variant(DefinitionFailure, _name)
# Codec is supplied by service after both namespaces have been initialized.
variant(DefinitionFailure, "InvalidLayout", [("code", int), ("codec", object)])


def _failure(value: JsDefinitionFailure) -> DefinitionFailure:
    from .._variants import enum_out
    from .service import Codec

    kinds = _jscatalog.DefinitionFailureKind
    if value.kind == kinds.Json:
        return DefinitionFailure.Json(value.line, value.column)
    if value.kind == kinds.Schema:
        return DefinitionFailure.Schema(value.schema)
    if value.kind == kinds.InvalidLayout:
        return DefinitionFailure.InvalidLayout(
            value.code, enum_out(value.codec, Codec, _jscatalog.service.Codec)
        )
    return {
        kinds.EmptyIdentity: DefinitionFailure.EmptyIdentity,
        kinds.InvalidDigest: DefinitionFailure.InvalidDigest,
        kinds.FirmwareMismatch: DefinitionFailure.FirmwareMismatch,
        kinds.EmptyModes: DefinitionFailure.EmptyModes,
        kinds.InvalidMailbox: DefinitionFailure.InvalidMailbox,
        kinds.SourceCount: DefinitionFailure.SourceCount,
        kinds.EmptyLabel: DefinitionFailure.EmptyLabel,
    }[value.kind]()


class FirmwareIdentity(Handle["JsFirmwareIdentity"]):
    def __init__(self, id: str, sha256: str) -> None:
        if type(id) is not str or type(sha256) is not str:
            raise TypeError("firmware identity requires strings")
        self._js = call(_jscatalog.FirmwareIdentity.new, id, sha256)

    @property
    def id(self) -> str:
        return self._js.id

    @property
    def sha256(self) -> str:
        return self._js.sha256


# Child definitions use FirmwareIdentity while this package initializes.

from . import service  # noqa: E402

__all__ = ["FirmwareIdentity", "DefinitionError", "DefinitionFailure", "service"]

if TYPE_CHECKING:
    from _fujimod_js import JsFirmwareIdentity as JsFirmwareIdentity
