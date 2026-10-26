"""Host callback protocols used by generated extension declarations."""

from typing import Protocol


class MemoryReader(Protocol):
    def __call__(self, address: int, size: int, *, cpu_id: int = 0) -> bytes: ...
