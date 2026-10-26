"""Control identities and badge geometry from the shared native recorder."""

import json
from typing import NotRequired, TypedDict, cast

from ..recording import controls


class Control(TypedDict):
    label: str
    text: NotRequired[str]
    icon: NotRequired[str]


class BadgeStyle(TypedDict):
    font_size: int
    icon_size: int
    padding_y: int
    padding_x: int
    border_width: int
    gap: int
    bottom: int
    radius: int
    release_ms: int
    background: str
    border: str
    foreground: str


class Shape(TypedDict):
    lines: NotRequired[list[list[tuple[int, int]]]]
    circle: NotRequired[tuple[int, int, int]]


class Presentation(TypedDict):
    controls: dict[str, Control]
    badge: BadgeStyle
    icons: dict[str, Shape]


SPEC = cast(Presentation, json.loads(controls()))


def control(name: str) -> Control:
    try:
        return SPEC["controls"][name]
    except KeyError:
        raise ValueError(f"Missing input presentation: {name}") from None
