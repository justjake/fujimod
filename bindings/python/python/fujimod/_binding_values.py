"""Language values generated from Rust schemas; ownership stays in Rust/Wasm."""

import json
from collections.abc import Buffer, Callable, Iterable
from enum import Enum
from inspect import Parameter, Signature
from typing import TYPE_CHECKING, Literal, Protocol, TypedDict, cast

from ._bridge import Handle, JsFree, buffer_in, buffer_out, call, sint, uint, unwrap
from ._variants import Sum, unwrap_variant, variant

if TYPE_CHECKING:
    from _fujimod_js import JsBytes


class IntegerType(TypedDict):
    kind: Literal["integer"]
    bits: int
    signed: bool


class NamedType(TypedDict):
    kind: Literal["named"]
    name: str


class BytesType(TypedDict):
    kind: Literal["bytes"]


class ArrayType(TypedDict):
    kind: Literal["array"]
    bits: int
    signed: bool
    length: int


type ValueType = IntegerType | NamedType | BytesType | ArrayType


class Field(TypedDict):
    name: str
    ty: ValueType


class Choice(TypedDict):
    name: str
    variants: list[str]
    result: bool
    kind_name: str


class QuantityParameter(TypedDict):
    name: str
    input: str


class Quantity(TypedDict):
    name: str
    parameters: list[QuantityParameter]
    getter: str


class Variant(TypedDict):
    name: str
    fields: list[Field]
    factory: str


class Union(TypedDict):
    kind_export: str
    name: str
    variants: list[Variant]
    input: bool
    kind_name: str


class Schema(TypedDict):
    choices: list[Choice]
    quantities: list[Quantity]
    unions: list[Union]


type Values = dict[str, type[object]]
type Decoder = Callable[[object], Sum]
type Decoders = dict[type[object], Decoder]


class KindValue(Protocol):
    kind: int


class UnknownValue(Protocol):
    rawStatus: int


class UnknownKind(Protocol):
    Unknown: int


class UnknownConstructor(Protocol):
    def Unknown(self, raw_status: int) -> Sum: ...


class JsValueQuantity(Protocol):
    value: int


class JsDataQuantity(Protocol):
    data: JsBytes


def _quantity_value(self: Handle[JsFree]) -> int:
    return cast(JsValueQuantity, self._js).value


def _quantity_data(self: Handle[JsFree]) -> bytes:
    return buffer_out(cast(JsDataQuantity, self._js).data)


def _choice_decoder(
    cls: type[Sum], kinds: object, constructors: dict[int, type[Sum]]
) -> Decoder:
    def decode(value: object) -> Sum:
        kind = cast(KindValue, value).kind
        if kind == cast(UnknownKind, kinds).Unknown:
            return cast(UnknownConstructor, cls).Unknown(
                cast(UnknownValue, value).rawStatus
            )
        return constructors[kind]()

    return decode


def bind_values(schema: str, js: object, module: str) -> tuple[Values, Decoders]:
    # This metadata is emitted by the same Rust build as the registered module.
    description = cast(Schema, json.loads(str(schema)))
    values: Values = {}
    decoders: Decoders = {}
    for choice in description["choices"]:
        name, members = choice["name"], choice["variants"]
        if not choice["result"]:
            values[name] = Enum(name, members, module=module)
            continue
        cls = cast(type[Sum], type(name, (Sum,), {"__module__": module}))
        for member in members:
            variant(cls, member)
        variant(cls, "Unknown", [("raw_status", int)])
        kinds = getattr(js, choice["kind_name"])
        constructors = {
            cast(int, getattr(kinds, member)): cast(type[Sum], getattr(cls, member))
            for member in members
        }
        values[name] = cls
        decoders[cls] = _choice_decoder(cls, kinds, constructors)
    for quantity in description["quantities"]:
        name = quantity["name"]
        constructor = cast(Callable[..., JsFree], getattr(js, name).new)
        initialize = _quantity_constructor(constructor, quantity["parameters"])
        if quantity["getter"] == "data":
            getter = property(_quantity_data)
            property_name = "data"
        else:
            getter = property(_quantity_value)
            property_name = "value"
        values[name] = type(
            name,
            (Handle,),
            {"__module__": module, "__init__": initialize, property_name: getter},
        )
    # Allocate payload classes before resolving references to other payloads.
    for union in description["unions"]:
        bases = (Sum, Handle) if union["input"] else (Sum,)
        values[union["name"]] = type(union["name"], bases, {"__module__": module})
    for union in description["unions"]:
        cls = cast(type[Sum], values[union["name"]])
        payloads: dict[int, tuple[type[Sum], list[Field]]] = {}
        for member in union["variants"]:
            fields = member["fields"]
            create = None
            if union["input"]:
                factory = cast(
                    Callable[..., JsFree],
                    getattr(getattr(js, union["name"]), member["factory"]),
                )
                create = _factory(factory, fields, values, js)
            constructor = variant(
                cls,
                member["name"],
                [(f["name"], _python_type(f["ty"], values)) for f in fields],
                create=create,
            )
            if not union["input"]:
                kind = getattr(getattr(js, union["kind_export"]), member["name"])
                payloads[kind] = (constructor, fields)
        if not union["input"]:
            decoders[cls] = _decoder(payloads, values, decoders, js)
    return values, decoders


def _quantity_constructor(
    constructor: Callable[..., JsFree], parameters: list[QuantityParameter]
) -> Callable[..., None]:
    signature = Signature([
        Parameter(p["name"], Parameter.POSITIONAL_OR_KEYWORD) for p in parameters
    ])

    def initialize(self: Handle[JsFree], *args: object, **kwargs: object) -> None:
        arguments = signature.bind(*args, **kwargs).arguments
        converted: list[object] = []
        for parameter in parameters:
            value = arguments[parameter["name"]]
            input_type = parameter["input"]
            if input_type == "bytes":
                converted.append(buffer_in(cast(Buffer, value)))
            else:
                bits = 32 if input_type == "Address" else int(input_type[1:])
                converted.append(
                    (sint if input_type.startswith("i") else uint)(
                        cast(int, value), bits
                    )
                )
        self._js = call(constructor, *converted)

    return initialize


def _python_type(ty: ValueType, values: Values) -> type[object]:
    if ty["kind"] == "integer":
        return int
    if ty["kind"] == "named":
        return values[ty["name"]]
    if ty["kind"] == "bytes" or (ty["bits"] == 8 and not ty["signed"]):
        return bytes
    return tuple


def _factory(
    factory: Callable[..., JsFree], fields: list[Field], values: Values, js: object
) -> Callable[..., JsFree]:
    def create(*arguments: object) -> JsFree:
        converted: list[object] = []
        for field, value in zip(fields, arguments, strict=True):
            ty = field["ty"]
            if ty["kind"] == "integer":
                converted.append(
                    (sint if ty["signed"] else uint)(cast(int, value), ty["bits"])
                )
                continue
            if ty["kind"] != "named":
                raise TypeError("input payload fields must be integers or named values")
            cls = values[ty["name"]]
            if issubclass(cls, Enum):
                if type(value) is not cls:
                    raise TypeError(f"expected {cls.__name__}")
                converted.append(getattr(getattr(js, ty["name"]), value.name))
            elif issubclass(cls, Handle):
                handle = cast(type[Handle[JsFree]], cls)
                converted.append(
                    unwrap_variant(value, handle)
                    if issubclass(cls, Sum)
                    else unwrap(value, handle)
                )
            else:
                raise TypeError(f"expected an owned {cls.__name__} handle")
        return call(factory, *converted)

    return create


def _decoder(
    constructors: dict[int, tuple[type[Sum], list[Field]]],
    values: Values,
    decoders: Decoders,
    js: object,
) -> Decoder:
    def decode(value: object) -> Sum:
        constructor, fields = constructors[cast(KindValue, value).kind]
        converted: list[object] = []
        for field in fields:
            name = field["name"].split("_")
            item = getattr(value, name[0] + "".join(part.title() for part in name[1:]))
            ty = field["ty"]
            if ty["kind"] == "named":
                cls = values[ty["name"]]
                if issubclass(cls, Enum):
                    choices = getattr(js, ty["name"])
                    item = {getattr(choices, m.name): m for m in cls}[item]
                elif issubclass(cls, Sum):
                    item = decoders[cls](item)
                else:
                    item = cast(type[Handle[JsFree]], cls)._wrap(item)
            elif ty["kind"] == "bytes" or (
                ty["kind"] == "array" and ty["bits"] == 8 and not ty["signed"]
            ):
                item = buffer_out(cast("JsBytes", item))
            elif ty["kind"] == "array":
                item = tuple(cast(Iterable[int], item))
            converted.append(item)
        return constructor(*converted)

    return decode
