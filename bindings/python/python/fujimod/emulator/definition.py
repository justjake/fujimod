"""Resolved execution facts and content identities supplied by a caller.

These values describe a checked execution graph. They do not select a camera
or locate files. Pass declared bytes to ``make_board`` or use
``Board.from_inputs`` for explicit paths to a catalog runtime's inputs.
"""

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Literal


@dataclass(frozen=True)
class Asset:
    """Identity and expected contents for one explicitly supplied input."""

    id: str
    size: int
    sha256: str

    def __post_init__(self) -> None:
        if (
            type(self.id) is not str
            or not self.id
            or len(self.id) > 128
            or not self.id[0].isascii()
            or not self.id[0].isalnum()
            or not all(
                character.isascii() and (character.isalnum() or character in "-_.")
                for character in self.id
            )
            or self.id in (".", "..")
        ):
            raise ValueError("Asset id must be a bounded identifier")
        if type(self.size) is not int or not 0 <= self.size < 1 << 64:
            raise ValueError("Asset size must be an unsigned 64-bit integer")
        if (
            type(self.sha256) is not str
            or len(self.sha256) != 64
            or any(character not in "0123456789abcdef" for character in self.sha256)
        ):
            raise ValueError(
                "Asset SHA-256 must contain 64 lowercase hexadecimal characters"
            )

    def manifest(self) -> dict[str, object]:
        return dict(id=self.id, content=dict(size=self.size, sha256=self.sha256))


@dataclass(frozen=True)
class Image:
    """Map an asset into physical memory or one processor's address space."""

    asset: str
    address: int
    core: int | None = None

    def __post_init__(self) -> None:
        if type(self.asset) is not str or not self.asset:
            raise ValueError("Image asset must name a declared input")
        if type(self.address) is not int or not 0 <= self.address < 1 << 64:
            raise ValueError("Image address must be an unsigned 64-bit integer")
        if self.core is not None and (
            type(self.core) is not int or not 0 <= self.core < 1 << 32
        ):
            raise ValueError("Image core must be an unsigned 32-bit integer")


@dataclass(frozen=True)
class Definition:
    """Resolved graph, device, and asset facts for a native emulator board."""

    execution: Mapping[str, object]
    devices: Mapping[str, object]
    assets: tuple[Asset, ...]
    images: tuple[Image, ...] = ()
    scene: Mapping[str, object] = field(default_factory=dict)
    engine: Literal["unicorn", "hypervisor"] = "unicorn"
    quantum: int = 1000
    mode: Literal["strict", "discovery"] = "strict"
    continue_refusals: bool = False
    adapters: Mapping[str, object] = field(default_factory=dict)


@dataclass(frozen=True)
class Xp4Definition:
    """Resolved facts for an XP4 execution board."""

    options: Mapping[str, object]
    assets: tuple[Asset, ...] = ()
    images: tuple[Image, ...] = ()
    storage: str | None = None
    quantum: int = 1000
