"""Semantic walkthrough text using Python's standard template strings.

Adapters resolve menu entries. The recorder exports resolved tokens and PNGs;
consumers need neither Python nor firmware to render the resulting document.
"""

import hashlib
import html
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass, replace
from pathlib import Path, PurePosixPath
from string.templatelib import Template, convert
from typing import Literal, TypedDict


class Icon(TypedDict):
    asset: str
    sha256: str


class TextToken(TypedDict):
    type: Literal["text"]
    text: str


class MenuToken(TypedDict):
    type: Literal["menu"]
    identity: str
    name: str
    display: str
    icon: Icon | None


class MenuPathToken(TypedDict):
    type: Literal["menu_path"]
    items: list[MenuToken]


type Token = TextToken | MenuToken | MenuPathToken


@dataclass(frozen=True)
class MenuReference:
    menu: str | int
    key: int | None = None
    display: str = "full"
    via: MenuReference | None = None

    def __post_init__(self) -> None:
        if not (
            (isinstance(self.menu, str) and self.menu)
            or type(self.menu) is int
            and self.menu >= 0
        ):
            raise ValueError("menu must be a symbol name or nonnegative menu code")
        if self.key is not None and (type(self.key) is not int or self.key < 0):
            raise ValueError("Menu entry key must be a nonnegative integer")
        if self.display not in ("full", "name", "icon"):
            raise ValueError("Unknown menu presentation")
        if self.via is not None and not isinstance(self.via, MenuReference):
            raise TypeError("via must reference the row that opens this menu")

    @property
    def name(self) -> MenuReference:
        return replace(self, display="name")

    @property
    def icon(self) -> MenuReference:
        return replace(self, display="icon")

    def path(self, *, relative_to: MenuReference | None = None) -> MenuPath:
        if relative_to is not None and not isinstance(relative_to, MenuReference):
            raise TypeError("relative_to must be a menu reference")
        return MenuPath(replace(self, display="full"), relative_to)

    def __str__(self) -> str:
        raise TypeError("Menu references need a t-string, not an f-string")


def ref(
    *, menu: str | int, key: int | None = None, via: MenuReference | None = None
) -> MenuReference:
    """Reference a declaration or stock menu; key selects a row in that menu."""
    return MenuReference(menu, key, via=via)


@dataclass(frozen=True)
class OptionReference:
    """One option of a menu row, by the label the camera shows."""

    item: MenuReference
    label: str

    def __post_init__(self) -> None:
        if not isinstance(self.item, MenuReference):
            raise TypeError("An option belongs to a menu reference")
        if not isinstance(self.label, str) or not self.label:
            raise ValueError("An option needs its label")


def option(item: MenuReference, label: str) -> OptionReference:
    """Reference one option of a menu row, e.g. option(ref(menu=...), '65:24')."""
    return OptionReference(item, label)


@dataclass(frozen=True)
class MenuPath:
    item: MenuReference
    relative_to: MenuReference | None = None

    def __str__(self) -> str:
        raise TypeError("Menu paths need a t-string, not an f-string")


@dataclass(frozen=True)
class MenuEntry:
    identity: str
    name: str
    icon: bytes | None = None
    ancestors: tuple[MenuEntry, ...] = ()


def encode(
    value: str | Template,
    *,
    resolve: Callable[[MenuReference], MenuEntry],
    output: Path,
) -> list[Token]:
    """Resolve str/Template into JSON tokens, retaining menu identity."""

    def menu(entry: MenuEntry, display: str = "full") -> MenuToken:
        token: MenuToken = dict(
            type="menu",
            identity=entry.identity,
            name=entry.name,
            display=display,
            icon=None,
        )
        if entry.icon is not None and display != "name":
            if not entry.icon.startswith(b"\x89PNG\r\n\x1a\n"):
                raise ValueError("Menu icons must be PNG bytes")
            digest = hashlib.sha256(entry.icon).hexdigest()
            asset = "icons/" + digest + ".png"
            destination = output / asset
            destination.parent.mkdir(exist_ok=True)
            if destination.exists() and destination.read_bytes() != entry.icon:
                raise ValueError("Conflicting menu icon asset")
            destination.write_bytes(entry.icon)
            token["icon"] = dict(asset=asset, sha256=digest)
        return token

    def interpolation(item: MenuReference | MenuPath) -> MenuToken | MenuPathToken:
        if isinstance(item, MenuReference):
            entry = resolve(item)
            if not entry.name and not entry.icon:
                raise ValueError(
                    "Menu has no visible caption; reference a displayed row"
                )
            return menu(entry, item.display)
        if isinstance(item, MenuPath):
            entry = resolve(item.item)
            entries = [*entry.ancestors, entry]
            if item.relative_to is not None:
                ancestor = resolve(item.relative_to)
                positions = [
                    i
                    for i, part in enumerate(entries[:-1])
                    if part.identity == ancestor.identity
                ]
                if len(positions) != 1:
                    raise ValueError("Relative menu path must name a unique ancestor")
                entries = entries[positions[0] + 1 :]
            entries = [part for part in entries if part.name or part.icon]
            if not entries:
                raise ValueError("Menu path has no visible entries")
            return MenuPathToken(
                type="menu_path", items=[menu(part) for part in entries]
            )
        raise TypeError("Unknown semantic walkthrough value")

    if isinstance(value, str):
        return [TextToken(type="text", text=value)] if value else []
    if not isinstance(value, Template):
        raise TypeError("Walkthrough text must be str or a t-string")
    tokens: list[Token] = []
    for part in value:
        if isinstance(part, str):
            tokens.append(TextToken(type="text", text=part))
        else:
            if isinstance(part.value, (MenuReference, MenuPath, Template)):
                if part.conversion or part.format_spec:
                    raise ValueError(
                        "Menu references use properties, not conversions or format specs"
                    )
                if isinstance(part.value, Template):
                    tokens.extend(encode(part.value, resolve=resolve, output=output))
                else:
                    tokens.append(interpolation(part.value))
            else:
                tokens.append(
                    TextToken(
                        type="text",
                        text=format(
                            convert(part.value, part.conversion), part.format_spec
                        ),
                    )
                )
    return tokens


def plain(tokens: Sequence[Token]) -> str:
    return "".join(
        token["text"]
        if token["type"] == "text"
        else " > ".join(item["name"] for item in token["items"])
        if token["type"] == "menu_path"
        else token["name"]
        for token in tokens
    )


def icons(tokens: Sequence[Token]) -> Iterator[Icon]:
    """Yield all icon descriptors; malformed token types are refused."""
    if not isinstance(tokens, list):
        raise ValueError("Walkthrough text must be a token array")
    for token in tokens:
        kind = token["type"]
        if token["type"] == "text":
            if not isinstance(token["text"], str):
                raise ValueError("Invalid text token")
        elif token["type"] == "menu_path":
            if not token["items"] or any(
                item["type"] != "menu" for item in token["items"]
            ):
                raise ValueError("Invalid menu path")
            yield from icons(token["items"])
        elif token["type"] == "menu":
            if (
                not isinstance(token["identity"], str)
                or not token["identity"]
                or not isinstance(token["name"], str)
                or token["display"] not in ("full", "name", "icon")
            ):
                raise ValueError("Invalid menu token")
            if token["icon"] is not None:
                icon = token["icon"]
                path = PurePosixPath(icon["asset"])
                if (
                    str(path) != icon["asset"]
                    or path.is_absolute()
                    or ".." in path.parts
                    or len(path.parts) != 2
                    or path.parts[0] != "icons"
                    or path.suffix != ".png"
                    or not isinstance(icon["sha256"], str)
                    or len(icon["sha256"]) != 64
                ):
                    raise ValueError("Invalid menu icon asset")
                yield icon
        else:
            raise ValueError("Unknown walkthrough text token: " + str(kind))


def render(tokens: Sequence[Token], *, prefix: str = "") -> str:
    """Render validated semantic text as escaped HTML, using bundled assets."""
    list(icons(tokens))

    def token_html(token: Token) -> str:
        if token["type"] == "text":
            return html.escape(token["text"])
        if token["type"] == "menu_path":
            return (
                '<span class="menu-path">'
                + '<span aria-hidden="true"> › </span>'.join(
                    token_html(item) for item in token["items"]
                )
                + "</span>"
            )
        label = html.escape(token["name"])
        image = ""
        if token["icon"] is not None and token["display"] != "name":
            alt = label if token["display"] == "icon" else ""
            image = f'<img class="menu-icon" src="{html.escape(prefix + token["icon"]["asset"], quote=True)}" alt="{alt}">'
        text = label if token["display"] != "icon" or not image else ""
        return f'<span class="menu-ref" data-menu="{html.escape(token["identity"], quote=True)}">{image}{text}</span>'

    return "".join(token_html(token) for token in tokens)
