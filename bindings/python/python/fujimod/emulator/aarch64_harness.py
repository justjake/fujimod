"""Explicit AArch64 preparation for checked, device-free routine execution."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from typing import Self

from fujimod._native import emulator as _native

from .board import make_board
from .definition import Asset, Definition, Image
from .driver_harness import _intersect, _merge, _subtract

Span = tuple[int, bytes]
Range = tuple[int, int]


def _range(begin: int, end: int) -> Range:
    if type(begin) is not int or type(end) is not int or not 0 <= begin < end < 1 << 64:
        raise ValueError("Expected an ordered unsigned 64-bit address range")
    return begin, end


def _word(value: int) -> int:
    if type(value) is not int or not 0 <= value < 1 << 64:
        raise ValueError("Expected an unsigned 64-bit register value")
    return value


def _receipt(address: int, data: bytes) -> dict:
    return {
        "address": address,
        "size": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def _contains(ranges: Iterable[Range], address: int, size: int) -> bool:
    return any(a <= address and address + size <= b for a, b in ranges)


@dataclass(frozen=True)
class Aarch64Image:
    name: str
    address: int
    data: bytes
    sha256: str
    executable: bool = True

    def __post_init__(self) -> None:
        if type(self.executable) is not bool:
            raise ValueError("Executable declaration must be a bool")
        Asset(self.name, len(self.data), self.sha256)
        if not isinstance(self.data, bytes) or not self.data:
            raise ValueError("Image requires immutable nonempty bytes")
        _range(self.address, self.address + len(self.data))
        if hashlib.sha256(self.data).hexdigest() != self.sha256:
            raise ValueError("Image SHA-256 differs: " + self.name)


@dataclass(frozen=True)
class Aarch64Inputs:
    images: tuple[Aarch64Image, ...]
    stack: Range
    stack_pointer: int
    return_address: int
    scratch: Range
    identity: Mapping[str, object] | None = None

    def __post_init__(self) -> None:
        if not self.images or len({i.name for i in self.images}) != len(self.images):
            raise ValueError("Choose distinct AArch64 image names")
        _range(*self.stack)
        _range(*self.scratch)
        _word(self.stack_pointer)
        _range(self.return_address, self.return_address + 4)
        if (
            self.stack_pointer & 15
            or not self.stack[0] < self.stack_pointer < self.stack[1]
        ):
            raise ValueError(
                "AArch64 stack requires 16-byte alignment inside its reservation"
            )
        if self.return_address & 3:
            raise ValueError("AArch64 return address requires 4-byte alignment")
        spans = sorted(
            [self.stack, self.scratch, (self.return_address, self.return_address + 4)]
            + [(i.address, i.address + len(i.data)) for i in self.images]
        )
        if any(a[1] > b[0] for a, b in zip(spans, spans[1:], strict=False)):
            raise ValueError("AArch64 images and caller reservations overlap")


class StubContext:
    """Argument snapshot and declared read/write effects for one substitution."""

    def __init__(self, arguments: list[int], snapshots: list[Span]) -> None:
        self._arguments, self._snapshots = arguments, snapshots
        self.writes: list[Span] = []

    def arg(self, index: int) -> int:
        if type(index) is not int or not 0 <= index < 8:
            raise ValueError("Stub argument index must be 0..7")
        return self._arguments[index]

    def read(self, address: int, size: int) -> bytes:
        _range(address, address + size)
        for base, data in self._snapshots:
            if base <= address and address + size <= base + len(data):
                return data[address - base : address - base + size]
        raise ValueError("Stub read requires a declared argument-relative snapshot")

    def u32(self, address: int) -> int:
        return int.from_bytes(self.read(address, 4), "little")

    def write(self, address: int, data: bytes) -> None:
        data = bytes(data)
        _range(address, address + len(data))
        self.writes.append((address, data))


class Aarch64DriverHarness:
    """Checked integer routines with fixed-return callee substitutions.

    Memory is sparse, flat, and explicit. Page padding is never readable or
    writable guest input. No reset, MMU, cache, peripheral or boot state is
    inferred. Unknown instruction families stop before execution.
    """

    def __init__(
        self,
        inputs: Aarch64Inputs,
        *,
        ram_fixture: Iterable[Span],
        patches: Iterable[Span] = (),
        quantum: int = 1000,
    ) -> None:
        if not isinstance(inputs, Aarch64Inputs):
            raise TypeError("AArch64 caller requires Aarch64Inputs")
        if type(quantum) is not int or quantum < 1:
            raise ValueError("Quantum must be positive")
        self.inputs, self.quantum = inputs, quantum
        self._fixture = [(a, bytes(data)) for a, data in ram_fixture]
        self._patches = [(a, bytes(data)) for a, data in patches]
        self._stubs: list[dict] = []
        self._stops: list[dict] = []
        self._observers: list[dict] = []
        self._handlers: dict[int, Callable[[StubContext], int]] = {}
        self._allocations: list[Span] = []
        self._alloc_next = inputs.scratch[0]
        self._closed = False
        self._failed = False
        self._image_ranges = [
            (i.address, i.address + len(i.data)) for i in inputs.images
        ]
        reserved = [
            *self._image_ranges,
            inputs.stack,
            inputs.scratch,
            (inputs.return_address, inputs.return_address + 4),
        ]
        fixture_ranges = []
        for address, data in self._fixture:
            span = _range(address, address + len(data))
            blocked = [
                inputs.scratch,
                (inputs.return_address, inputs.return_address + 4),
            ]
            if not _contains([inputs.stack], address, len(data)):
                blocked.append(inputs.stack)
            if any(span[0] < b and a < span[1] for a, b in blocked + fixture_ranges):
                raise ValueError("RAM fixture overlaps an image or caller reservation")
            fixture_ranges.append(span)
        for address, data in self._patches:
            _range(address, address + len(data))
            if (
                address & 3
                or len(data) & 3
                or not _contains(self._image_ranges, address, len(data))
            ):
                raise ValueError(
                    "Code patch requires aligned words within a declared image"
                )
        pages = _merge([
            (a & ~0x3FFF, (b + 0x3FFF) & ~0x3FFF) for a, b in reserved + fixture_ranges
        ])
        assets = {image.name: image.data for image in inputs.images}
        definition = Definition(
            execution={
                "processors": [
                    {
                        "model": "armv8a",
                        "released": True,
                        "mprofile": None,
                        "rprofile": None,
                        "registers": [
                            ["Pstate", 0x3C5],
                            [
                                {
                                    "Arm64System": {
                                        "op0": 3,
                                        "op1": 0,
                                        "crn": 1,
                                        "crm": 0,
                                        "op2": 2,
                                    }
                                },
                                3 << 20,
                            ],
                        ],
                    }
                ],
                "ram": [{"base": a, "size": b - a} for a, b in pages],
                "mappings": [
                    {
                        "core": 0,
                        "address": a,
                        "physical": a,
                        "size": b - a,
                        "permissions": 7,
                    }
                    for a, b in pages
                ],
                "dma_mappings": [
                    {"address": a, "physical": a, "size": b - a, "permissions": 3}
                    for a, b in pages
                ],
                "peripherals": [],
                "decoders": [],
                "instructions_per_tick": [1, 1],
            },
            devices={"kind": "none"},
            assets=tuple(Asset(i.name, len(i.data), i.sha256) for i in inputs.images),
            images=tuple(Image(i.name, i.address) for i in inputs.images),
            quantum=quantum,
        )
        self._execution = dict(definition.execution)
        self.board = make_board(definition, assets)
        try:
            for address, data in self._patches:
                self.board.write_memory(address, data)
            self._expected_images = [
                (i.address, self.read(i.address, len(i.data))) for i in inputs.images
            ]
            for address, data in self._fixture:
                self.board.write_memory(address, data)
        except BaseException:
            self.board.close()
            raise

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def close(self) -> None:
        if not self._closed:
            self.board.close()
            self._closed = True

    def read(self, address: int, size: int) -> bytes:
        return self.board.read_memory(address, size)

    def alloc(self, data: bytes | int, *, alignment: int = 16) -> int:
        if type(alignment) is not int or alignment < 1 or alignment & (alignment - 1):
            raise ValueError("Allocation alignment must be a positive power of two")
        if type(data) is int:
            if data < 1 or data > self.inputs.scratch[1] - self.inputs.scratch[0]:
                raise ValueError("Invalid allocation size")
            data = bytes(data)
        data = bytes(data)
        address = (self._alloc_next + alignment - 1) & -alignment
        if not data or address + len(data) > self.inputs.scratch[1]:
            raise ValueError("Allocation exceeds explicit scratch reservation")
        self.board.write_memory(address, data)
        self._allocations.append((address, data))
        self._alloc_next = address + len(data)
        return address

    def stub(
        self,
        target: int,
        *,
        returns: int | Callable[[StubContext], int] = 0,
        label: str = "",
        reads: Iterable[tuple[int, int | str | dict[str, int]]] = (),
    ) -> None:
        if (
            type(target) is not int
            or target & 3
            or not _contains(self._image_ranges, target, 4)
        ):
            raise ValueError("Stub requires an aligned address inside a declared image")
        if not isinstance(label, str) or any(
            s["address"] == target for s in self._stubs
        ):
            raise ValueError("Stub needs a unique address and text label")
        reads = tuple(reads)
        if any(
            type(i) is not int
            or not 0 <= i < 8
            or not (
                type(n) is int
                and 0 < n <= 1 << 20
                or isinstance(n, str)
                and n in {f"x{r}" for r in range(8)}
                or isinstance(n, dict)
                and set(n) == {"indirect", "size"}
                and type(n["indirect"]) is int
                and 0 <= n["indirect"] <= 1 << 20
                and type(n["size"]) is int
                and 0 < n["size"] <= 1 << 20
            )
            for i, n in reads
        ):
            raise ValueError(
                "Stub reads require argument 0..7 and a bounded size or x0..x7 size register"
            )
        reads = tuple(
            (i, {"argument": int(n[1:])} if isinstance(n, str) else n) for i, n in reads
        )
        dynamic = callable(returns)
        if dynamic:
            self._handlers[target] = returns
        self._stubs.append({
            "address": target,
            "returns": 0 if dynamic else _word(returns),
            "label": label,
            "reads": reads,
            "dynamic": dynamic,
        })

    def stop_at(self, address: int, *, label: str = "") -> None:
        if type(address) is not int or address & 3 or not isinstance(label, str):
            raise ValueError("Stop requires an aligned address and text label")
        if any(stop["address"] == address for stop in self._stops):
            raise ValueError("Stop address already declared")
        self._stops.append({"address": address, "label": label})

    def observe_at(self, address: int, *, label: str = "") -> None:
        """Record architectural registers before an admitted instruction, then continue."""
        if type(address) is not int or address & 3 or not isinstance(label, str):
            raise ValueError("Observer requires an aligned address and text label")
        if any(observer["address"] == address for observer in self._observers):
            raise ValueError("Observer address already declared")
        self._observers.append({"address": address, "label": label})

    def _callback(
        self, address: int, arguments: list[int], snapshots: list[Span]
    ) -> tuple[int, list[Span]]:
        context = StubContext(arguments, snapshots)
        value = _word(self._handlers[address](context))
        return value, context.writes

    def call(
        self,
        entry: int,
        *,
        args: Iterable[int] = (),
        max_instructions: int = 100000,
        allowed_code: Iterable[Range] | None = None,
        allowed_stores: Iterable[Range] | None = None,
        sp: int | None = None,
        registers: Mapping[str, int] | None = None,
        store_limit: int = 1 << 20,
        fragment_exit: Mapping[str, int] | None = None,
        observer_limit: int = 4096,
        system_reads: Iterable[tuple[str, int]] = (),
    ) -> dict:
        """Execute a routine with optional expected CPU-model system reads.

        Each ``system_reads`` pair names a read-only architectural register and
        its expected model value. Results include initial model values and MRS
        observations; for x31 the value is the model value because XZR discards
        the result. Values describe the execution model, not physical hardware.
        """
        if self._closed or self._failed:
            raise RuntimeError("Create a fresh caller after closing or a failed call")
        args = tuple(_word(value) for value in args)
        if len(args) > 8 or type(max_instructions) is not int or max_instructions < 1:
            raise ValueError(
                "AArch64 accepts x0-x7 arguments and a positive instruction limit"
            )
        if type(entry) is not int or entry & 3:
            raise ValueError("Entry requires an aligned AArch64 address")
        sp = self.inputs.stack_pointer if sp is None else _word(sp)
        if sp & 15 or not self.inputs.stack[0] < sp < self.inputs.stack[1]:
            raise ValueError("Stack requires 16-byte alignment inside its reservation")
        default_code = [
            (i.address, i.address + len(i.data))
            for i in self.inputs.images
            if i.executable
        ]
        executable = _merge(
            default_code if allowed_code is None else [_range(*r) for r in allowed_code]
        )
        if not executable or any(
            a & 3 or b & 3 or not _contains(self._image_ranges, a, b - a)
            for a, b in executable
        ):
            raise ValueError("Allowed code must be aligned spans of declared images")
        if not _contains(executable, entry, 4):
            raise ValueError("Initial entry must lie within declared executable code")
        if any(
            a < address + len(data) and address < b
            for a, b in executable
            for address, data in self._fixture
        ):
            raise ValueError("RAM fixture overlaps executable bytes")
        for a, b in executable:
            base, expected = next(
                (base, data)
                for base, data in self._expected_images
                if base <= a and b <= base + len(data)
            )
            if self.read(a, b - a) != expected[a - base : b - base]:
                raise ValueError("Code image differs from declared bytes and patches")
        reg_values = []
        for name, value in (registers or {}).items():
            if name not in {f"x{n}" for n in range(31)}:
                raise ValueError("Initial registers must name x0..x30")
            reg_values.append((int(name[1:]), _word(value)))
        readable = _merge(
            self._image_ranges
            + [(a, a + len(d)) for a, d in self._fixture + self._allocations]
            + [self.inputs.stack]
        )
        writable = _subtract(
            _merge(
                [(a, a + len(d)) for a, d in self._fixture + self._allocations]
                + [self.inputs.stack]
            ),
            executable,
        )
        if allowed_stores is not None:
            writable = _merge(
                _intersect(writable, [_range(*r) for r in allowed_stores])
                + [self.inputs.stack]
            )
        options = {
            "stop_addresses": [s["address"] for s in self._stops],
            "observe_addresses": [o["address"] for o in self._observers],
            "observer_limit": observer_limit,
            "system_reads": [(name, _word(value)) for name, value in system_reads],
            "entry": entry,
            "arguments": args,
            "return_address": self.inputs.return_address,
            "stack_pointer": sp,
            "stack": self.inputs.stack,
            "max_instructions": max_instructions,
            "quantum": self.quantum,
            "executable": executable,
            "readable": readable,
            "writable": writable,
            "store_limit": store_limit,
            "stubs": self._stubs,
            "registers": reg_values,
            "fragment_exit": dict(fragment_exit or {}),
        }
        result = json.loads(
            self.board.native.call_aarch64(json.dumps(options), self._callback)
        )
        self._failed = result["failure"] is not None
        result.update(
            images=[
                dict(name=i.name, **_receipt(i.address, i.data))
                for i in self.inputs.images
            ],
            code=[_receipt(a, self.read(a, b - a)) for a, b in executable],
            patches=[_receipt(a, d) for a, d in self._patches],
            ram_fixture=[_receipt(a, d) for a, d in self._fixture],
            allocations=[_receipt(a, d) for a, d in self._allocations],
            stubs=list(self._stubs),
            stops=list(self._stops),
            observers=list(self._observers),
            allowed_code=[list(r) for r in executable],
            allowed_stores=[list(r) for r in writable],
            readable=[list(r) for r in readable],
            caller=options,
            profile=dict(self.inputs.identity or {}),
            execution=json.loads(json.dumps(self._execution)),
            native_identity={
                "version": _native.VERSION,
                "unicorn_abi": _native.UNICORN_ABI,
                "target": _native.TARGET,
                "rustc": _native.RUSTC_VERSION,
            },
            runtime="shared",
            architecture="aarch64",
            vector_initial_state="v0-v31, FPCR and FPSR initialized to zero for each call",
            limits="Isolated synthetic caller state; no MMU, cache, device, reset, boot, timing or hardware success established",
        )
        return result
