"""Caller-supplied labels and function ranges, independent of storage format."""

from collections.abc import Sequence
from typing import Protocol


class SymbolLookup(Protocol):
    def entries(self, name: str) -> Sequence[int]: ...

    def ranges(self, entry: int) -> Sequence[tuple[int, int]] | None: ...

    def containing(self, address: int) -> tuple[int, str] | None: ...
