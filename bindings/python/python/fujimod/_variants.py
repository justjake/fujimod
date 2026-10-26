"""Python representations of payload enums returned by the Rust bindings."""

from collections.abc import Callable, Sequence
from dataclasses import Field, field, make_dataclass
from enum import Enum
from typing import ClassVar, Self, cast

from ._bridge import Handle


class Sum:
    __match_args__: ClassVar[tuple[str, ...]]

    def __new__(cls, *args: object, **kwargs: object) -> Self:
        if not cls.__dict__.get("_is_variant", False):
            raise TypeError(f"construct a named {cls.__name__} variant")
        return object.__new__(cls)


def variant[T: Sum](
    base: type[T],
    name: str,
    fields: Sequence[tuple[str, object]] = (),
    *,
    create: Callable[..., object] | None = None,
) -> type[T]:
    """A frozen variant, optionally owning a Rust enum through a JS handle.

    The factory converts language values only; it never encodes protocol bytes.
    """
    namespace: dict[str, object] = {
        "__module__": base.__module__,
        "__qualname__": f"{base.__qualname__}.{name}",
        "_is_variant": True,
    }
    normalized_fields: list[tuple[str, object] | tuple[str, object, Field[object]]] = (
        list(fields)
    )
    if create is not None:
        names = [name for name, _ in fields]

        def initialize(self: T) -> None:
            object.__setattr__(
                self, "_js", create(*(getattr(self, name) for name in names))
            )

        namespace["__post_init__"] = initialize
        normalized_fields.append(("_js", object, field(init=False, repr=False)))
    result = make_dataclass(
        name,
        normalized_fields,
        bases=(base,),
        namespace=namespace,
        frozen=True,
        eq=False,
    )
    setattr(base, name, result)
    return cast(type[T], result)


def enum_out[E: Enum](value: int, enum: type[E], choices: object) -> E:
    return {getattr(choices, member.name): member for member in enum}[value]


def unwrap_variant[T](value: object, cls: type[Handle[T]]) -> T:
    if not isinstance(value, cls):
        raise TypeError(f"expected {cls.__name__} variant")
    return cast(Handle[T], value)._js
