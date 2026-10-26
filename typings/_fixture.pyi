from collections.abc import Callable, Iterator
from typing import Protocol

class Configuration(Protocol):
    def set(self, key: str, value: int) -> None: ...

class State(Protocol):
    hangControl: int | None
    kind: int
    configuration: Configuration
    onConfigurationWrite: Callable[[int, int, bytes], None] | None

class Log(Protocol):
    def to_py(self) -> list[list[object]]: ...
    def __iter__(self) -> Iterator[list[object]]: ...

state: State
log: Log
