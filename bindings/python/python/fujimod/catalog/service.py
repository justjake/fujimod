"""Language views of the shared Rust service definition, without USB commands."""

from typing import TYPE_CHECKING, Self

from _fujimod_js import catalog as _jscatalog
from _fujimod_js import valueSchemas as _schemas

from .._binding_values import bind_values as _bind_values
from .._bridge import Handle, buffer_out, call, uint, unwrap
from .._variants import Sum, enum_out, variant
from . import FirmwareIdentity

if TYPE_CHECKING:
    from _fujimod_js import (
        JsServiceDefinition,
        JsServiceEntry,
        JsServiceMailbox,
        JsServiceModes,
    )


_js = _jscatalog.service


if TYPE_CHECKING:
    from _fujimod_service_values import Codec as Codec
else:
    Codec = _bind_values(_schemas.definitions, _js, "fujimod.catalog.service")[0][
        "Codec"
    ]


class ServiceModes(Handle["JsServiceModes"]):
    def __init__(self, mask: int) -> None:
        self._js = call(_js.ServiceModes.new, uint(mask))

    @property
    def mask(self) -> int:
        return self._js.mask

    def __eq__(self, other: object) -> bool:
        return type(other) is ServiceModes and self.mask == other.mask


if TYPE_CHECKING:
    from .._definition_types import ModeAvailability as ModeAvailability
else:

    class ModeAvailability(Sum):
        pass


variant(ModeAvailability, "Modes", [("modes", ServiceModes)])
variant(ModeAvailability, "Disabled")
variant(ModeAvailability, "Unreviewed")


if TYPE_CHECKING:
    from .._definition_types import ModeTransition as ModeTransition
else:

    class ModeTransition(Sum):
        pass


variant(ModeTransition, "Keep")
variant(ModeTransition, "Set", [("modes", ServiceModes)])


class Entry(Handle["JsServiceEntry"]):
    def __init__(self) -> None:
        raise TypeError("entries come from a loaded definition")

    @property
    def code(self) -> int:
        return self._js.code

    @property
    def label(self) -> str:
        return self._js.label

    @property
    def codec(self) -> Codec:
        return enum_out(self._js.codec, Codec, _js.Codec)

    @property
    def slots(self) -> tuple[int, ...]:
        return tuple(buffer_out(self._js.slots))

    @property
    def firmware_kind(self) -> int:
        return self._js.firmwareKind

    @property
    def handler(self) -> int:
        return int(self._js.handler)

    @property
    def parameter_table(self) -> int:
        return int(self._js.parameterTable)

    @property
    def enabled_modes(self) -> ModeAvailability:
        value = self._js.enabledModes
        kinds = _js.ModeAvailabilityKind
        if value.kind == kinds.Modes:
            return ModeAvailability.Modes(ServiceModes._wrap(value.modes))
        return {
            kinds.Disabled: ModeAvailability.Disabled,
            kinds.Unreviewed: ModeAvailability.Unreviewed,
        }[value.kind]()

    @property
    def transition(self) -> ModeTransition:
        value = self._js.transition
        if value.kind == _js.ModeTransitionKind.Set:
            return ModeTransition.Set(ServiceModes._wrap(value.modes))
        return ModeTransition.Keep()


class Mailbox(Handle["JsServiceMailbox"]):
    def __init__(self) -> None:
        raise TypeError("mailbox metadata comes from a loaded definition")

    @property
    def page(self) -> int:
        return self._js.page

    @property
    def offset(self) -> int:
        return self._js.offset

    @property
    def configuration_limit(self) -> int:
        return self._js.configurationLimit


class Definition(Handle["JsServiceDefinition"]):
    def __init__(self) -> None:
        raise TypeError("use Definition.from_json")

    @classmethod
    def from_json(cls, json: str, expected: FirmwareIdentity) -> Self:
        if type(json) is not str:
            raise TypeError("definition JSON must be a string")
        return cls._wrap(
            call(_js.Definition.fromJson, json, unwrap(expected, FirmwareIdentity))
        )

    @property
    def identity(self) -> FirmwareIdentity:
        return FirmwareIdentity._wrap(self._js.identity)

    @property
    def model(self) -> str:
        return self._js.model

    @property
    def version(self) -> str:
        return self._js.version

    @property
    def mailbox(self) -> Mailbox:
        return Mailbox._wrap(self._js.mailbox)

    @property
    def entries(self) -> list[Entry]:
        return [Entry._wrap(value) for value in self._js.entries]

    def select(self, code: int, modes: ServiceModes) -> Entry | None:
        value = call(self._js.select, uint(code, 8), unwrap(modes, ServiceModes))
        return None if value is None else Entry._wrap(value)


if TYPE_CHECKING:
    from _fujimod_js import JsServiceDefinition as JsServiceDefinition
    from _fujimod_js import JsServiceEntry as JsServiceEntry
    from _fujimod_js import JsServiceMailbox as JsServiceMailbox
    from _fujimod_js import JsServiceModes as JsServiceModes
