"""Resolved code and caller reservations for isolated ARM routine execution."""

import hashlib
from dataclasses import dataclass
from typing import Protocol

from fujimod._native import emulator as _native

from .harness_memory import MemoryLayout
from .processor import Processor
from .symbols import SymbolLookup


class CallerBoard(Protocol):
    """Paused board services used by the ARM caller, on the common Session."""

    cpus: tuple[Processor, ...] | list[Processor]
    identity: list[int]
    selected_core: int
    quantum: int
    transaction_limit: int
    executing_cpu: int | None
    failure: dict[str, object] | None
    transactions: list[dict[str, object]]
    device_graph: dict[str, object]
    input_manifest: dict[str, object]

    @property
    def native(self) -> _native.Board: ...
    @property
    def instructions(self) -> int: ...
    @property
    def virtual_ticks(self) -> int: ...
    def read_memory(self, address: int, size: int, cpu_id: int = 0) -> bytes: ...
    def write_memory(self, address: int, data: bytes, cpu_id: int = 0) -> None: ...
    def state(self) -> dict[str, object]: ...
    def attach(self, device: object) -> object: ...
    def _stop(
        self, cpu: Processor, core: int, message: str, **details: object
    ) -> None: ...


@dataclass(frozen=True)
class CodeImage:
    name: str
    address: int
    data: bytes
    sha256: str
    readonly: bool = False

    def __post_init__(self) -> None:
        if not isinstance(self.name, str) or not self.name:
            raise ValueError("Code image needs a name")
        if (
            type(self.address) is not int
            or not isinstance(self.data, bytes)
            or not self.data
            or not 0 <= self.address < self.address + len(self.data) <= 1 << 32
        ):
            raise ValueError("Code image needs bytes within a 32-bit address interval")
        if hashlib.sha256(self.data).hexdigest() != self.sha256:
            raise ValueError("Code image SHA-256 differs: " + self.name)
        if type(self.readonly) is not bool:
            raise ValueError("Code image readonly must be a bool")


@dataclass(frozen=True)
class CallerLayout:
    return_address: int
    stack_pointer: int
    scratch: tuple[int, int]

    def __post_init__(self) -> None:
        lo, hi = self.scratch
        if (
            any(
                type(value) is not int
                for value in (self.return_address, self.stack_pointer, lo, hi)
            )
            or self.return_address & 3
            or self.stack_pointer & 7
            or not 0 <= self.return_address <= (1 << 32) - 4
            or not 0 <= self.stack_pointer < 1 << 32
            or not 0 <= lo < hi <= 1 << 32
        ):
            raise ValueError(
                "Caller layout needs aligned ARM return/stack and an ordered scratch interval"
            )


@dataclass(frozen=True)
class HarnessInputs:
    board: CallerBoard
    memory: MemoryLayout
    images: tuple[CodeImage, ...]
    reserved_images: tuple[tuple[int, int], ...]
    caller: CallerLayout
    symbols: SymbolLookup
    observed_memory: tuple[tuple[int, int], ...]
    irq_cores: tuple[int, ...] = ()
    identity: dict[str, object] | None = None

    def __post_init__(self) -> None:
        if not self.images or len({image.name for image in self.images}) != len(
            self.images
        ):
            raise ValueError("Choose distinct mapped image names")
        images = sorted(
            (image.address, image.address + len(image.data)) for image in self.images
        )
        if any(a[1] > b[0] for a, b in zip(images, images[1:], strict=False)):
            raise ValueError("Mapped code images overlap")
        for lo, hi in (*self.reserved_images, *self.observed_memory):
            if (
                type(lo) is not int
                or type(hi) is not int
                or not 0 <= lo < hi <= 1 << 32
            ):
                raise ValueError("Caller memory needs ordered 32-bit address intervals")
        if len(set(self.irq_cores)) != len(self.irq_cores) or any(
            type(core) is not int or not 0 <= core < len(self.board.cpus)
            for core in self.irq_cores
        ):
            raise ValueError(
                "IRQ vector declaration must name distinct declared processors"
            )
        caller = self.caller
        for address, size in (
            (caller.return_address, 4),
            (caller.stack_pointer - 0x1000, 0x1040),
            (caller.scratch[0], caller.scratch[1] - caller.scratch[0]),
        ):
            if self.memory.physical(address) != address:
                raise ValueError("Caller reservations must use physical DRAM addresses")
            if not self.memory.contains_dram(address, size):
                raise ValueError("Caller reservations must lie in declared DRAM")
        reservations = sorted(
            (self.memory.physical(address), self.memory.physical(address) + size)
            for address, size in (
                (caller.return_address, 4),
                (caller.stack_pointer - 0x1000, 0x1040),
                (caller.scratch[0], caller.scratch[1] - caller.scratch[0]),
            )
        )
        if any(
            a[1] > b[0] for a, b in zip(reservations, reservations[1:], strict=False)
        ):
            raise ValueError("Caller return, stack and scratch reservations overlap")

    def observation_policy(self) -> dict[str, object]:
        return dict(
            aliases=[
                dict(address=a, physical=p, size=n) for a, p, n in self.memory.aliases
            ],
            observed=list(self.observed_memory),
        )
