"""Isolated ARM routine execution with declared code, RAM and substitutions.

Calls use ordinary ARM registers and stack arguments, bounded native execution,
exact-address instrumentation and optional fetch/store/coverage guards. Host
preparation supplies every image, memory view and symbol provider explicitly.
No file lookup or firmware selection occurs in this module.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable, Iterable, Sequence
from typing import Literal, TypedDict, TypeGuard

from . import registers as regs
from .harness_inputs import HarnessInputs
from .harness_memory import MemoryLayout
from .processor import Processor
from .registers import CPSR, LR, NAMES, PC, R0, SP
from .symbols import SymbolLookup

STACK_BELOW = 0x1000
STACK_ABOVE = 64
STORE_LIMIT = 1 << 20
WORD = 1 << 32
CALL_MODES = (0x10, 0x11, 0x12, 0x13, 0x17, 0x1B, 0x1F)
# KernelInterrupts trace records that are IRQ assertions or deliveries.
IRQ_REQUEST_RECORDS = (
    "Software IRQ request",
    "Peripheral IRQ request",
    "Architectural IRQ entry",
)
FINAL_REGISTERS = tuple(f"r{index}" for index in range(13)) + ("sp", "lr", "pc", "cpsr")
FAULT_RECORDS = (
    "Architectural MPU permission prefetch abort entry",
    "Architectural undefined instruction entry",
)

Range = tuple[int, int]
ByteBuffer = bytes | bytearray | memoryview
Span = tuple[int, ByteBuffer]
DataOverride = tuple[int, ByteBuffer, str]
Target = int | str
BreakpointKind = Literal["stub", "observe", "stop", "substitute"]
Handler = Callable[["CallContext"], None]
StubReturn = int | Callable[["CallContext"], int | None]


class FunctionSummary(TypedDict):
    entry: int | None
    name: str | None
    pcs: int
    first_pc: int
    last_pc: int


def _is_json_object(value: object) -> TypeGuard[dict[str, object]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in value)


def _json_object(value: object) -> dict[str, object]:
    if not _is_json_object(value):
        raise RuntimeError("Native observation must be a string-keyed object")
    return value


def _json_objects(value: object) -> list[dict[str, object]]:
    if not isinstance(value, list):
        raise RuntimeError("Native observation must be a list of objects")
    return [_json_object(item) for item in value]


def _is_integer_list(value: object) -> TypeGuard[list[int]]:
    return isinstance(value, list) and all(type(item) is int for item in value)


def _integers(value: object) -> list[int]:
    if not _is_integer_list(value):
        raise RuntimeError("Native observation must be a list of integers")
    return value


def _integer(value: object) -> int:
    if type(value) is not int:
        raise RuntimeError("Native observation field must be an integer")
    return value


def _text(value: object) -> str:
    if not isinstance(value, str):
        raise RuntimeError("Native observation field must be text")
    return value


def _word(value: int, what: str) -> int:
    if type(value) is not int or not -(1 << 31) <= value < WORD:
        raise ValueError(what + " must be a 32-bit machine word")
    return value & 0xFFFFFFFF


def _merge(ranges: Sequence[Range]) -> list[Range]:
    merged = []
    for lo, hi in sorted(ranges):
        if merged and lo <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], hi))
        else:
            merged.append((lo, hi))
    return merged


def _intersect(ranges: Sequence[Range], limits: Sequence[Range]) -> list[Range]:
    result = []
    for lo, hi in ranges:
        for base, top in limits:
            if max(lo, base) < min(hi, top):
                result.append((max(lo, base), min(hi, top)))
    return _merge(result)


def _subtract(ranges: Sequence[Range], holes: Sequence[Range]) -> list[Range]:
    result = []
    for lo, hi in ranges:
        pieces = [(lo, hi)]
        for hole_lo, hole_hi in holes:
            pieces = [
                part
                for piece in pieces
                for part in (
                    (piece,)
                    if hole_hi <= piece[0] or piece[1] <= hole_lo
                    else tuple(
                        item
                        for item in ((piece[0], hole_lo), (hole_hi, piece[1]))
                        if item[0] < item[1]
                    )
                )
            ]
        result.extend(pieces)
    return _merge(result)


class CallContext:
    """Breakpoint view of the caller state: valid only inside its handler."""

    def __init__(
        self,
        harness: DriverHarness,
        cpu: Processor,
        pc: int,
        label: str,
        kind: BreakpointKind,
    ) -> None:
        self._harness, self._cpu, self.pc, self.label, self.kind = (
            harness,
            cpu,
            pc,
            label,
            kind,
        )
        self.returned = None
        self.stopped = None
        self._live = True

    def _check(self) -> None:
        if not self._live:
            raise RuntimeError(
                "Breakpoint context is only valid while its handler runs"
            )

    def reg(self, name: str) -> int:
        self._check()
        if name not in NAMES:
            raise ValueError("Unknown register: " + str(name))
        return self._cpu.reg_read(NAMES[name])

    def set_reg(self, name: str, value: int) -> None:
        self._check()
        if name not in NAMES:
            raise ValueError("Unknown register: " + str(name))
        self._cpu.reg_write(NAMES[name], _word(value, "Register value"))

    def arg(self, index: int) -> int:
        """Argument ``index``: r0-r3, then caller stack words at SP (hit-time)."""
        self._check()
        if type(index) is not int or not 0 <= index < 16:
            raise ValueError("Argument index must be 0..15")
        if index < 4:
            return self._cpu.reg_read(regs.GENERAL[index])
        return self.u32(self._cpu.reg_read(SP) + 4 * (index - 4))

    def read(self, address: int, size: int) -> bytes:
        self._check()
        return self._harness.board.read_memory(address, size, self._harness._core)

    def u32(self, address: int) -> int:
        return int.from_bytes(self.read(address, 4), "little")

    def write(self, address: int, data: ByteBuffer) -> None:
        """Coherent bus write (also invalidates cached translations)."""
        self._check()
        data = bytes(data)
        if (
            self._harness._memory.physical(address) < self._harness._return_address + 4
            and self._harness._return_address
            < self._harness._memory.physical(address) + len(data)
        ):
            raise ValueError("Breakpoint write covers the reserved return address")
        self._harness.board.write_memory(address, data, self._harness._core)

    def ret(self, value: int | None = None) -> None:
        """Observe-style early return: r0=value (if given) and PC=LR on exit."""
        self._check()
        if self.kind != "observe":
            raise RuntimeError(
                "ret() is only available to observe handlers; stubs return automatically"
            )
        if value is not None:
            _word(value, "Return value")
        self.returned = (value,)

    def stop(self, reason: str | None = None) -> None:
        """End the call before this instruction runs (like ``stop_at``).

        Valid in stub, observe and stop handlers. The instruction does not
        execute and is not counted; a stub's return value is discarded. The
        result records ``stopped_at`` and ``stop`` (pc, label, reason).
        """
        self._check()
        if reason is not None and not isinstance(reason, str):
            raise ValueError("Stop reason must be a string")
        self.stopped = (reason,)


class DriverHarness:
    """Isolated ARM caller over an explicitly prepared board and code images.

    Images carry checked hashes and read-only declarations. Memory views,
    scratch/stack reservations and symbol lookup are caller inputs. Fixtures,
    patches, payloads and callee substitutions remain explicit in every result.
    The board owns device construction and uses the common native Session.
    """

    def __init__(
        self,
        inputs: HarnessInputs,
        *,
        ram_fixture: Iterable[Span],
        cpu: int = 0,
        transaction_limit: int = 4096,
        patches: Iterable[Span] = (),
        payloads: Iterable[Span] = (),
        fetch_guard: bool = False,
        data_overrides: Iterable[DataOverride] = (),
        cpu_identity: dict[int, int] | None = None,
    ) -> None:
        if not isinstance(inputs, HarnessInputs):
            raise TypeError("inputs must be resolved HarnessInputs")
        self.board = inputs.board
        if type(cpu) is not int or not 0 <= cpu < len(self.board.cpus):
            raise ValueError("Driver CPU must name a declared processor")
        if type(transaction_limit) is not int or transaction_limit < 1:
            raise ValueError("Driver transaction bound must be a positive integer")
        if type(fetch_guard) is not bool:
            raise ValueError("fetch_guard must be a bool")
        identity = self._identity_map(cpu_identity)
        self._symbols: SymbolLookup = inputs.symbols
        self._profile_identity = (
            None if inputs.identity is None else dict(inputs.identity)
        )
        self._memory = inputs.memory
        self._observation_memory = inputs.observation_policy()
        self._irq_cores = inputs.irq_cores
        self._return_address = inputs.caller.return_address
        self._stack_pointer = inputs.caller.stack_pointer
        self._alloc_base, self._alloc_limit = inputs.caller.scratch
        self.board.transaction_limit = transaction_limit
        self.cpu = self._core = cpu
        self.fetch_guard = fetch_guard
        self._default_identity = tuple(self.board.identity)
        for core, value in identity.items():
            self.board.identity[core] = value
        self._base_identity = tuple(self.board.identity)
        self._select_core(cpu)
        for image in inputs.images:
            if (
                image.readonly
                and self.board.read_memory(image.address, len(image.data)) != image.data
            ):
                raise ValueError(
                    "Read-only image differs from declared bytes: " + image.name
                )
        self.images, self.image_names, self._readonly_images = [], [], []
        for image in inputs.images:
            if not image.readonly:
                self.board.write_memory(image.address, image.data)
            self.images.append((image.address, image.data))
            self.image_names.append(image.name)
            self._readonly_images.append(image.readonly)
        self._manifest_ranges = list(inputs.reserved_images)
        self.patches, self.payloads, self.allocations, self.data_overrides = (
            [],
            [],
            [],
            [],
        )
        self._fixture_ranges, self._alloc_next = [], self._alloc_base
        self._breakpoints, self._hits, self._stopped_at, self._hit_limit = (
            {},
            [],
            None,
            0,
        )
        self._stop_info = None
        self.ram_fixture_spans = []
        self.data_override_events, self.payload_replacements = [], []
        self._override_originals, self._calls = {}, 0
        self._apply_patches(patches)
        # Overrides come before the fixture: a capture that overlaps overridden
        # image bytes is validated against the overridden bytes, not the image.
        for override in data_overrides:
            try:
                address, data, reason = override
            except TypeError, ValueError:
                raise ValueError(
                    "A data override is (address, bytes, reason)"
                ) from None
            self.data_override(address, data, reason)
        self._declare_payloads(payloads)
        self.load_ram(ram_fixture)

    @property
    def memory(self) -> MemoryLayout:
        """The caller's declared RAM views; inspecting them does not run the board."""
        return self._memory

    # -- helpers ---------------------------------------------------------
    def _identity_map(self, identity: dict[int, int] | None) -> dict[int, int]:
        if identity is None:
            return {}
        if not isinstance(identity, dict):
            raise ValueError("cpu_identity is {core: 32-bit value}")
        for core, value in identity.items():
            if type(core) is not int or not 0 <= core < len(self.board.cpus):
                raise ValueError("cpu_identity names a nonexistent core: " + str(core))
            if type(value) is not int or not 0 <= value < WORD:
                raise ValueError("cpu_identity values are unsigned 32-bit words")
        return dict(identity)

    def _select_core(self, core: int) -> None:
        """Only ``core`` is runnable: the other stays parked for the call."""
        self.board.selected_core = self._core = core

    # -- declared state -------------------------------------------------
    def _apply_patches(self, patches: Iterable[Span]) -> None:
        prepared = []
        for item in patches:
            try:
                address, data = item
            except TypeError, ValueError:
                raise ValueError("A patch is (address, bytes)") from None
            if (
                type(address) is not int
                or not isinstance(data, (bytes, bytearray, memoryview))
                or not len(data)
            ):
                raise ValueError("A patch needs an integer address and nonempty bytes")
            data = bytes(data)
            owner = [
                index
                for index, (base, image) in enumerate(self.images)
                if base <= address and address + len(data) <= base + len(image)
            ]
            if not owner:
                raise ValueError(
                    "Patch must lie wholly inside one chosen mapped image: "
                    + hex(address)
                )
            if self._readonly_images[owner[0]]:
                raise ValueError("A read-only image cannot be patched")
            prepared.append((address, data, owner[0]))
        ordered = sorted(prepared, key=lambda patch: patch[0])
        if any(
            a[0] + len(a[1]) > b[0] for a, b in zip(ordered, ordered[1:], strict=False)
        ):
            raise ValueError("Patches must not overlap")
        for address, data, index in ordered:
            base, image = self.images[index]
            original = image[address - base : address - base + len(data)]
            patched = (
                image[: address - base] + data + image[address - base + len(data) :]
            )
            self.board.write_memory(address, data)
            self.images[index] = (base, patched)
            self.patches.append(
                dict(
                    address=address,
                    length=len(data),
                    section=self.image_names[index],
                    original_sha256=hashlib.sha256(original).hexdigest(),
                    patched_sha256=hashlib.sha256(data).hexdigest(),
                )
            )

    def data_override(self, address: int, data: ByteBuffer, reason: str) -> int:
        """Declare runtime data written inside a chosen mapped image.

        For globals/flags that live inside a section and for capture-vs-image
        differences; not code (use ``patches`` for code). Reported separately
        as ``data_overrides``. The bytes may not overlap a patch or another
        override, and an executed instruction that fetches from an overridden
        byte fails the call (checked natively through coverage; the fetch guard
        also excludes them) for as long as the override is active;
        ``data_override_restore`` ends it. ``data_overrides`` lists the active
        overrides, ``data_override_events`` every override and restore in order.
        Returns the address.
        """
        if self.board.executing_cpu is not None:
            raise ValueError("data_override requires a paused harness")
        if (
            type(address) is not int
            or not isinstance(data, (bytes, bytearray, memoryview))
            or not len(data)
        ):
            raise ValueError(
                "A data override needs an integer address and nonempty bytes"
            )
        if not isinstance(reason, str) or not reason.strip():
            raise ValueError("A data override needs a reason")
        data = bytes(data)
        owner = [
            index
            for index, (base, image) in enumerate(self.images)
            if base <= address and address + len(data) <= base + len(image)
        ]
        if not owner:
            raise ValueError(
                "Data override must lie wholly inside one chosen mapped image: "
                + hex(address)
            )
        index = owner[0]
        if self._readonly_images[index]:
            raise ValueError("A read-only image cannot be overridden")
        spans = [
            (p["address"], p["address"] + p["length"])
            for p in self.patches + self.data_overrides
        ]
        if any(address < hi and lo < address + len(data) for lo, hi in spans):
            raise ValueError("Data override overlaps a patch or another override")
        base, image = self.images[index]
        original = image[address - base : address - base + len(data)]
        self.board.write_memory(address, data)
        self.images[index] = (
            base,
            image[: address - base] + data + image[address - base + len(data) :],
        )
        record = dict(
            address=address,
            length=len(data),
            section=self.image_names[index],
            reason=reason,
            original_sha256=hashlib.sha256(original).hexdigest(),
            new_sha256=hashlib.sha256(data).hexdigest(),
        )
        self.data_overrides.append(record)
        self._override_originals[address] = original
        self.data_override_events.append(
            dict(record, event="override", call_index=self._calls)
        )
        return address

    def data_override_restore(self, address: int) -> int:
        """Put the original image bytes back under the active override at ``address``.

        ``address`` is the exact start address of an active override. The image
        bytes (and the mapped memory) return to what they were before the
        override; the override leaves ``data_overrides`` and, once none remain,
        the executed-fetch check stops applying. The restore is appended to
        ``data_override_events`` (``event='restore'``, with ``call_index`` the
        number of calls made before it). Requires a paused harness. Returns
        the address.
        """
        if self.board.executing_cpu is not None:
            raise ValueError("data_override_restore requires a paused harness")
        matches = [item for item in self.data_overrides if item["address"] == address]
        if type(address) is not int or not matches:
            raise ValueError("No active data override starts at " + str(address))
        record = matches[0]
        original = self._override_originals.pop(address)
        index = self.image_names.index(record["section"])
        base, image = self.images[index]
        length = record["length"]
        self.board.write_memory(address, original)
        self.images[index] = (
            base,
            image[: address - base] + original + image[address - base + length :],
        )
        self.data_overrides.remove(record)
        self.data_override_events.append(
            dict(
                record,
                event="restore",
                call_index=self._calls,
                restored_sha256=hashlib.sha256(original).hexdigest(),
                overridden_sha256=record["new_sha256"],
            )
        )
        return address

    def replace_payload(
        self, address: int, data: ByteBuffer, *, reason: str
    ) -> dict[str, object]:
        """Rewrite bytes inside one already-declared payload, between calls.

        ``data`` replaces exactly ``len(data)`` bytes at ``address`` (physical
        or a declared alias), which must lie wholly inside a single declared
        payload; the payload keeps its length and any bytes outside the
        replaced span (a shorter replacement is not zero-padded). Anything
        outside a declared payload is refused (use ``write`` for fixtures and
        allocations, ``patches``/``data_override`` for images). Translation
        caches for all declared memory views are invalidated;
        registered breakpoints stay registered. Each replacement is appended to
        ``payload_replacements`` (also in every call result): ``address``,
        ``length``, ``old_sha256``, ``new_sha256`` of the span,
        ``payload`` (its address), ``payload_sha256_before/after``, ``reason``
        and ``call_index`` (the number of calls made before it). ``payloads``
        entries report the current whole-payload ``sha256``. Returns the record.
        """
        if self.board.executing_cpu is not None:
            raise ValueError("replace_payload requires a paused harness")
        if (
            type(address) is not int
            or not isinstance(data, (bytes, bytearray, memoryview))
            or not len(data)
        ):
            raise ValueError(
                "A payload replacement needs an integer address and nonempty bytes"
            )
        if not isinstance(reason, str) or not reason.strip():
            raise ValueError("A payload replacement needs a reason")
        data, physical = bytes(data), self._memory.physical(address)
        owner = [
            item
            for item in self.payloads
            if item["address"] <= physical
            and physical + len(data) <= item["address"] + item["length"]
        ]
        if not owner:
            raise ValueError(
                "Replacement must lie wholly inside one declared payload: "
                + hex(address)
            )
        payload = owner[0]
        old = self.board.read_memory(physical, len(data), self._core)
        before = payload["sha256"]
        self.board.write_memory(physical, data, self._core)
        payload["sha256"] = hashlib.sha256(
            self.board.read_memory(payload["address"], payload["length"], self._core)
        ).hexdigest()
        record = dict(
            address=physical,
            length=len(data),
            old_sha256=hashlib.sha256(old).hexdigest(),
            new_sha256=hashlib.sha256(data).hexdigest(),
            payload=payload["address"],
            payload_sha256_before=before,
            payload_sha256_after=payload["sha256"],
            reason=reason,
            call_index=self._calls,
        )
        self.payload_replacements.append(record)
        return dict(record)

    def reg(self, name: str, cpu: int | None = None) -> int:
        """Read one register (``registers.NAMES`` or banked name) of ``cpu`` between calls."""
        if self.board.executing_cpu is not None:
            raise ValueError("reg requires a paused harness")
        return regs.read_register(self.board.cpus[self._core_arg(cpu)], name)

    def _declare_payloads(self, payloads: Iterable[Span]) -> None:
        prepared = []
        for item in payloads:
            try:
                address, data = item
            except TypeError, ValueError:
                raise ValueError("A payload is (address, bytes)") from None
            if (
                type(address) is not int
                or not isinstance(data, (bytes, bytearray, memoryview))
                or not len(data)
            ):
                raise ValueError(
                    "A payload needs an integer address and nonempty bytes"
                )
            data, physical = bytes(data), self._memory.physical(address)
            if not self._memory.contains_dram(address, len(data)):
                raise ValueError("Payload is outside modeled DRAM")
            if any(
                physical < hi and lo < physical + len(data)
                for lo, hi in self._manifest_ranges
            ):
                raise ValueError(
                    "Payload overlaps a mapped firmware section; use patches for mapped bytes: "
                    + hex(physical)
                )
            if (
                physical < self._return_address + 4
                and self._return_address < physical + len(data)
            ):
                raise ValueError("Payload covers the reserved return address")
            if physical < self._alloc_limit and self._alloc_base < physical + len(data):
                raise ValueError("Payload overlaps the alloc() scratch window")
            self._check_stack_window(
                self._stack_pointer, physical, physical + len(data), "Payload"
            )
            if any(
                physical < hi and lo < physical + len(data)
                for lo, hi in self._fixture_ranges
            ):
                raise ValueError("Payload overlaps a RAM fixture")
            prepared.append((physical, data))
        ordered = sorted(prepared)
        if any(
            a[0] + len(a[1]) > b[0] for a, b in zip(ordered, ordered[1:], strict=False)
        ):
            raise ValueError("Payloads must not overlap")
        for physical, data in ordered:
            self.board.write_memory(physical, data)
            self.payloads.append(
                dict(
                    address=physical,
                    length=len(data),
                    sha256=hashlib.sha256(data).hexdigest(),
                )
            )

    @staticmethod
    def _check_stack_window(
        stack_pointer: int, lo: int, hi: int, what: str, below: int = STACK_BELOW
    ) -> None:
        if lo < stack_pointer + STACK_ABOVE and stack_pointer - below < hi:
            raise ValueError(what + " overlaps the caller stack window")

    def _code_ranges(self) -> list[Range]:
        return [(base, base + len(image)) for base, image in self.images] + [
            (item["address"], item["address"] + item["length"])
            for item in self.payloads
        ]

    def load_ram(self, spans: Iterable[Span]) -> None:
        """Preflight explicit DRAM fixtures before applying any span."""
        self._load_ram(spans, True)

    def _load_ram(self, spans: Iterable[Span], declare: bool) -> None:
        prepared = []
        for address, data in spans:
            if type(address) is not int or not isinstance(
                data, (bytes, bytearray, memoryview)
            ):
                raise ValueError("RAM fixture needs an integer address and bytes")
            data = bytes(data)
            physical = self._memory.physical(address)
            if not self._memory.contains_dram(address, len(data)):
                raise ValueError("RAM fixture is outside modeled DRAM")
            if (
                physical < self._return_address + 4
                and self._return_address < physical + len(data)
            ):
                raise ValueError("RAM fixture covers the reserved return address")
            for base, image in self.images:
                lo, hi = (
                    max(base, physical),
                    min(base + len(image), physical + len(data)),
                )
                if (
                    lo < hi
                    and data[lo - physical : hi - physical]
                    != image[lo - base : hi - base]
                ):
                    raise ValueError("RAM fixture changes mapped firmware bytes")
            if data and any(
                physical < lo + size and lo < physical + len(data)
                for lo, size in (
                    (p["address"], p["length"])
                    for p in self.payloads + self.allocations
                )
            ):
                raise ValueError(
                    "RAM fixture overlaps a declared payload or allocation"
                )
            prepared.append((physical, data))
        for address, data in prepared:
            if data:
                self.board.write_memory(address, data)
                if declare:
                    self._fixture_ranges.append((address, address + len(data)))
            self.ram_fixture_spans.append(dict(address=address, bytes=len(data)))

    def alloc(
        self, size: int, *, align: int = 8, data: ByteBuffer | None = None
    ) -> int:
        """Zero-filled (or ``data``-initialized) scratch DRAM; returns its address.

        Bump allocation within the explicitly reserved scratch interval. Refused if it would
        overlap a fixture, payload or image. Allocations persist across calls
        and are listed in every result; ``write()`` may change their bytes
        between calls.
        """
        if (
            type(size) is not int
            or not 0 < size <= self._alloc_limit - self._alloc_base
        ):
            raise ValueError(
                "Allocation size must be a positive integer within the scratch window"
            )
        if type(align) is not int or align < 1 or align & (align - 1) or align > 0x1000:
            raise ValueError("Allocation alignment must be a power of two up to 4096")
        if data is not None:
            if not isinstance(data, (bytes, bytearray, memoryview)) or len(data) > size:
                raise ValueError(
                    "Allocation data must be bytes no longer than the allocation"
                )
            data = bytes(data)
        address = (self._alloc_next + align - 1) & -align
        if address + size > self._alloc_limit:
            raise ValueError("Scratch window exhausted")
        spans = (
            [(lo, hi) for lo, hi in self._fixture_ranges]
            + self._manifest_ranges
            + [(p["address"], p["address"] + p["length"]) for p in self.payloads]
        )
        if any(address < hi and lo < address + size for lo, hi in spans):
            raise ValueError(
                "Scratch allocation would overlap an image, payload or fixture"
            )
        self.board.write_memory(
            address, bytes(size) if data is None else data.ljust(size, b"\0")
        )
        self._alloc_next = address + size
        self.allocations.append(
            dict(
                address=address,
                length=size,
                align=align,
                initialized_bytes=0 if data is None else len(data),
            )
        )
        return address

    # -- memory API ------------------------------------------------------
    def _memory_class(self, core: int, address: int, size: int) -> str | None:
        """'dram', 'sram', 'vectors' or 'tcm' when [address, address+size) is
        wholly one modeled RAM region of ``core``; None otherwise (MMIO, unmapped)."""
        return self._memory.classify(core, address, size)

    def _mapped_rw(self, core: int, lo: int, hi: int) -> bool:
        """True when [lo, hi) lies in merged readable+writable regions of ``core``."""
        return self._memory_class(core, lo, hi - lo) is not None

    def _core_arg(self, cpu: int | None) -> int:
        core = self.cpu if cpu is None else cpu
        if type(core) is not int or not 0 <= core < len(self.board.cpus):
            raise ValueError("cpu must name a declared processor")
        return core

    def read(self, address: int, size: int, *, cpu: int | None = None) -> bytes:
        """Read modeled RAM (DRAM, SRAM, vector pages, ``cpu``'s TCM); never MMIO."""
        core = self._core_arg(cpu)
        if self.board.executing_cpu is not None:
            raise ValueError("read requires a paused harness")
        kind = self._memory_class(core, address, size)
        if kind is None or (
            kind in ("tcm", "vectors")
            and not self._mapped_rw(core, address, address + size)
        ):
            raise ValueError(
                f"{address:#x}+{size} is not modeled RAM (DRAM, SRAM or core {core} TCM)"
            )
        return self.board.read_memory(address, size, core)

    def write(self, address: int, data: ByteBuffer, *, cpu: int | None = None) -> None:
        """Coherent write to modeled RAM between calls (invalidates caches).

        DRAM (either alias), SRAM, vector pages and ``cpu``'s TCM. Refused over
        chosen mapped images (use ``patches`` for code, ``data_override`` for
        data inside a section), payloads and the reserved return address.
        Declared fixtures and ``alloc()`` buffers may be written freely.
        """
        core = self._core_arg(cpu)
        if self.board.executing_cpu is not None:
            raise ValueError("write requires a paused harness")
        if not isinstance(data, (bytes, bytearray, memoryview)) or not len(data):
            raise ValueError("write needs nonempty bytes")
        data = bytes(data)
        kind = self._memory_class(core, address, len(data))
        if kind is None or (
            kind in ("tcm", "vectors")
            and not self._mapped_rw(core, address, address + len(data))
        ):
            raise ValueError(
                f"{address:#x}+{len(data)} is not modeled RAM (DRAM, SRAM or core {core} TCM)"
            )
        if kind == "dram":
            physical = self._memory.physical(address)
            end = physical + len(data)
            if any(
                physical < base + len(image) and base < end
                for base, image in self.images
            ):
                raise ValueError(
                    "write would change a mapped image; use patches or data_override"
                )
            if any(
                physical < p["address"] + p["length"] and p["address"] < end
                for p in self.payloads
            ):
                raise ValueError("write would change a declared payload")
            if physical < self._return_address + 4 and self._return_address < end:
                raise ValueError("write covers the reserved return address")
        self.board.write_memory(address, data, core)

    def attach(self, device: object) -> object:
        """Declare a NativeNode before the first CPU operation; no Python models."""
        return self.board.attach(device)

    def _resolve(self, entry: Target) -> tuple[str | None, int]:
        label = None
        if isinstance(entry, str):
            try:
                entry = int(entry, 16)
            except ValueError:
                matches = self._symbols.entries(entry)
                if len(matches) != 1:
                    raise ValueError(
                        "Choose an exact unique declared function name"
                    ) from None
                label, entry = entry, matches[0]
        if type(entry) is not int or entry & 1:
            raise ValueError(
                "Driver entry must be an aligned address in a chosen mapped image or payload"
            )
        return label, self._memory.physical(entry)

    def _entry(self, entry: Target) -> int:
        entry = self._resolve(entry)[1]
        if entry & 3 or not any(lo <= entry < hi for lo, hi in self._code_ranges()):
            raise ValueError(
                "Driver entry must be an aligned address in a chosen mapped image or payload"
            )
        return entry

    def functions(self, executed: Iterable[int]) -> list[FunctionSummary]:
        """Map executed PCs using the injected function labels and ranges.

        Returns ``[{'entry', 'name', 'pcs': n, 'first_pc', 'last_pc'}, ...]``
        sorted by entry; PCs outside every declared function form a final record
        with ``entry`` and ``name`` None.
        """
        found, unknown = {}, []
        for pc in executed:
            hit = self._symbols.containing(pc)
            if hit is None:
                unknown.append(pc)
            else:
                found.setdefault(hit, []).append(pc)
        result: list[FunctionSummary] = [
            FunctionSummary(
                entry=entry,
                name=name,
                pcs=len(pcs),
                first_pc=min(pcs),
                last_pc=max(pcs),
            )
            for (entry, name), pcs in sorted(found.items())
        ]
        if unknown:
            result.append(
                FunctionSummary(
                    entry=None,
                    name=None,
                    pcs=len(unknown),
                    first_pc=min(unknown),
                    last_pc=max(unknown),
                )
            )
        return result

    def _function_ranges(self, functions: Iterable[Target]) -> list[Range]:
        functions = tuple(functions)
        if not functions:
            return []  # No symbol query unless a function was actually named.
        result: list[Range] = []
        for item in functions:
            if isinstance(item, str):
                try:
                    entry = int(item, 16)
                except ValueError:
                    matches = self._symbols.entries(item)
                    if len(matches) != 1:
                        raise ValueError(
                            "allowed_functions needs an exact unique declared function name: "
                            + item
                        ) from None
                    entry = matches[0]
            else:
                entry = item
            if type(entry) is not int:
                raise ValueError(
                    "allowed_functions entry is not a declared function: " + str(item)
                )
            ranges = self._symbols.ranges(entry)
            if ranges is None:
                raise ValueError(
                    "allowed_functions entry is not a declared function: " + str(item)
                )
            result.extend(ranges)
        return result

    # -- breakpoints -----------------------------------------------------
    def _register(
        self, target: Target, kind: BreakpointKind, label: str | None, **action: object
    ) -> int:
        name, address = self._resolve(target)
        if not any(lo <= address < hi for lo, hi in self._code_ranges()):
            raise ValueError(
                "Breakpoint target must be an aligned address in a chosen mapped image or payload"
            )
        if address in self._breakpoints:
            raise ValueError("Breakpoint target is already registered: " + hex(address))
        if len(self._breakpoints) >= 4096:
            raise ValueError("Too many breakpoints")
        self._breakpoints[address] = dict(
            kind=kind, label=label or name or f"{address:#010x}", **action
        )
        return address

    def stub(
        self, target: Target, returns: StubReturn = 0, *, label: str | None = None
    ) -> int:
        """Replace a callee: its first instruction never runs; PC=LR, r0=returns.

        ``returns`` is an int (r0), or a callable ``fn(ctx)`` yielding an int
        or None (r0 unchanged). LR with the Thumb bit set fails the call.
        """
        if not callable(returns):
            _word(returns, "Stub return value")
        return self._register(target, "stub", label, returns=returns)

    def observe(self, target: Target, fn: Handler, *, label: str | None = None) -> int:
        """Call ``fn(ctx)`` at ``target``, then run the instruction normally."""
        if not callable(fn):
            raise ValueError("Observer must be callable")
        return self._register(target, "observe", label, fn=fn)

    def stop_at(
        self, target: Target, fn: Handler | None = None, *, label: str | None = None
    ) -> int:
        """End the call successfully (returned=False) before ``target`` runs.

        The instruction at ``target`` neither executes nor counts in
        ``instructions``. ``fn(ctx)``, if given, runs first (observe and stop
        on one address share this single registration); ``ctx.stop(reason)``
        from any handler has the same effect.

        A stop at the call's own entry PC is not armed until execution has
        left the entry (at least one instruction executed, an interrupt
        injected, or an exception taken), like Unicorn's ``until``. So
        ``call(P)`` with ``stop_at(P)`` and a pending IRQ injects at the start,
        runs the ISR and stops when execution returns to P; with nothing
        pending it runs from P until control returns to P. A stop at any other
        PC fires before the next IRQ injection.
        """
        if fn is not None and not callable(fn):
            raise ValueError("stop_at handler must be callable")
        return self._register(target, "stop", label, fn=fn)

    def substitute(
        self, target: Target, fn: Handler, *, label: str | None = None
    ) -> int:
        """Replace one ARM instruction's effect: ``fn(ctx)`` runs instead of it.

        Firmware bytes stay unchanged; the instruction at ``target`` neither
        executes nor counts in ``instructions``, and execution continues at
        ``target + 4``. ``fn`` supplies the effect through ``ctx.set_reg`` /
        ``ctx.write`` (e.g. a CP15 ID read the engine cannot answer) and must
        honour the instruction's condition itself. Each hit is reported with
        the replaced instruction word. Thumb state fails the call.
        """
        if not callable(fn):
            raise ValueError("Substitution handler must be callable")
        return self._register(target, "substitute", label, fn=fn)

    def unstub(self, target: Target) -> int:
        """Drop one registered stub; refuses other kinds. Returns its address."""
        address = self._resolve(target)[1]
        record = self._breakpoints.get(address)
        if record is None or record["kind"] != "stub":
            raise ValueError("No stub is registered at " + hex(address))
        del self._breakpoints[address]
        return address

    def remove(self, target: Target) -> int:
        """Drop whatever breakpoint (stub, observer or stop) is registered at ``target``."""
        address = self._resolve(target)[1]
        if address not in self._breakpoints:
            raise ValueError("No breakpoint is registered at " + hex(address))
        del self._breakpoints[address]
        return address

    def clear_breakpoints(self) -> None:
        self._breakpoints.clear()

    def _breakpoint(self, cpu: Processor, core: int, address: int) -> None:
        board, record = self.board, self._breakpoints.get(address)
        if record is None:
            board._stop(
                cpu,
                core,
                "Native breakpoint at an unregistered address",
                pc=hex(address),
            )
            return
        if len(self._hits) >= self._hit_limit:
            board._stop(cpu, core, "Isolated driver breakpoint-hit bound reached")
            return
        kind, lr = record["kind"], cpu.reg_read(LR)
        hit = dict(
            pc=address,
            label=record["label"],
            kind=kind,
            args=tuple(cpu.reg_read(register) for register in regs.GENERAL[:4]),
            return_value=None,
        )
        self._hits.append(hit)
        context = CallContext(self, cpu, address, record["label"], kind)
        value = None
        try:
            if kind == "stub":
                value = (
                    record["returns"](context)
                    if callable(record["returns"])
                    else record["returns"]
                )
                if value is not None:
                    _word(value, "Stub return value")
            elif record.get("fn") is not None:
                record["fn"](context)
                if kind == "observe" and context.returned is not None:
                    hit["kind"], value = "observe_return", context.returned[0]
        finally:
            context._live = False
        if context.stopped is not None or kind == "stop":
            if cpu.reg_read(PC) != address:
                board._stop(
                    cpu,
                    core,
                    f"Breakpoint handler {record['label']} changed PC; use stub() or ret()",
                    pc=hex(address),
                )
                return
            reason = context.stopped[0] if context.stopped is not None else None
            self._stopped_at = address
            self._stop_info = dict(
                pc=address, label=record["label"], reason=reason, kind=kind
            )
            if reason is not None:
                hit["stop_reason"] = reason
            # Not cpu.emu_stop(): the native hook must also skip counting and
            # executing the instruction.
            return
        if cpu.reg_read(PC) != address:
            board._stop(
                cpu,
                core,
                f"Breakpoint handler {record['label']} changed PC; use stub() or ret()",
                pc=hex(address),
            )
            return
        if kind == "substitute":
            if cpu.reg_read(regs.CPSR) & 0x20:
                board._stop(
                    cpu,
                    core,
                    "Instruction substitution requires ARM state",
                    pc=hex(address),
                )
                return
            hit["word"] = int.from_bytes(board.read_memory(address, 4, core), "little")
            cpu.reg_write(PC, address + 4)
            return
        if (kind == "stub" or hit["kind"] == "observe_return") and lr & 1:
            board._stop(
                cpu,
                core,
                "Stub return address has the Thumb bit set",
                lr=hex(lr),
                pc=hex(address),
            )
            return
        if kind == "stub" or hit["kind"] == "observe_return":
            if value is not None:
                cpu.reg_write(R0, value & 0xFFFFFFFF)
                hit["return_value"] = value & 0xFFFFFFFF
            else:
                hit["return_value"] = cpu.reg_read(R0)
            cpu.reg_write(PC, lr)

    # -- execution -------------------------------------------------------
    def advance(self, ticks: int) -> dict[str, object]:
        """Advance guest time with both callers parked and native deadlines active."""
        board = self.board
        if type(ticks) is not int or ticks < 0:
            raise ValueError("advance needs a nonnegative integer tick count")
        if board.executing_cpu is not None:
            raise ValueError("advance requires a paused harness")
        if board.failure:
            raise ValueError("Cannot advance a failed isolated driver")
        board.native.driver_advance(board.virtual_ticks + ticks, board.quantum)
        return dict(
            virtual_ticks=board.virtual_ticks,
            pending_scheduled_completions=self._pending_completions(),
        )

    def _pending_completions(self) -> int:
        timeline = _json_object(self.board.state()["timeline"])
        events = timeline["events"]
        if not isinstance(events, list):
            raise RuntimeError("Board timeline events must be a list")
        return len(events)

    def enable_flat_mpu(self, cpu: int | None = None) -> None:
        """Program MPU region 0 as 4 GiB, full access, executable; set SCTLR.M.

        The default (MPU-disabled) PMSAv7 map makes addresses at or above
        0x80000000 execute-never, so calling code in the ``rom`` image needs an
        explicit region. Only region 0 and the SCTLR.M bit change (other SCTLR
        bits and regions are kept); the selected region number is restored.
        This is declared caller CPU state, not boot-time MPU behavior.
        """
        core = self._core_arg(cpu)
        if self.board.executing_cpu is not None:
            raise ValueError("enable_flat_mpu requires a paused harness")
        target = self.board.cpus[core]
        selected = target.cpr_read(15, 0, 6, 2, 0, 0, False)
        try:
            target.cpr_write(15, 0, 6, 2, 0, 0, False, 0)
            for crm, opc2, value in ((1, 0, 0), (1, 2, 0x3F), (1, 4, 0x303)):
                target.cpr_write(15, 0, 6, crm, opc2, 0, False, value)
        finally:
            target.cpr_write(15, 0, 6, 2, 0, 0, False, selected)
        target.cpr_write(
            15, 0, 1, 0, 0, 0, False, target.cpr_read(15, 0, 1, 0, 0, 0, False) | 1
        )

    def _guard_configuration(
        self, cpu: Processor, core: int, ranges: Sequence[Range], deliver_faults: bool
    ) -> tuple[list[Range], int]:
        vectors: list[Range] = []
        opcode = 0xE59FF018
        needed = []
        sctlr = cpu.cpr_read(15, 0, 1, 0, 0, 0, False)
        base = 0xFFFF0000 if sctlr & (1 << 13) else 0
        if core in self._irq_cores:
            needed.append(base + 0x18)
        if deliver_faults:
            needed.extend((
                base + 4,
                base + 0xC,
                base + 0x10,
            ))  # undefined, prefetch/data abort
        words = set()
        for vector in needed:
            # Exception entry fetches the vector word, which may live in the
            # loader image (low vectors) or a copied high vector. Permit exactly
            # that word, and only while it is an LDR PC,[PC,#imm] dispatch.
            word = int.from_bytes(self.board.read_memory(vector, 4, core), "little")
            if (
                not any(lo <= vector and vector + 4 <= hi for lo, hi in ranges)
                and word & 0xFFFFF000 == 0xE59FF000
            ):
                vectors.append((vector, vector + 4))
                words.add(word)
        if len(words) > 1:
            raise ValueError(
                "Fetch guard vector words differ; one dispatch opcode is allowed per call"
            )
        if words:
            opcode = words.pop()
        return vectors, opcode

    def _plan_stack(
        self, core: int, stack_pointer: int, stack_bytes: int, align_sp: bool
    ) -> tuple[Literal["dram", "tcm"], Range, int]:
        """Validate SP for ``core``; returns (space, store-log window)."""
        if stack_pointer & 3 or (align_sp and stack_pointer & 7):
            raise ValueError(
                "Driver stack needs aligned DRAM or TCM with room for arguments"
            )
        below = max(stack_bytes, STACK_BELOW)
        physical = self._memory.physical(stack_pointer)
        in_dram = self._memory.contains_dram(stack_pointer - below, below + STACK_ABOVE)
        if in_dram:
            window = (physical - below, physical + STACK_ABOVE)
            if (
                window[0] < self._return_address + 4
                and self._return_address < window[1]
            ):
                raise ValueError(
                    "Driver caller stack window covers the reserved return address"
                )
            if any(
                physical - 0x1000 < base + len(image) and base < physical + 64
                for base, image in self.images
            ):
                raise ValueError("Driver caller stack overlaps a mapped image")
            for item in self.payloads + self.allocations:
                self._check_stack_window(
                    physical,
                    item["address"],
                    item["address"] + item["length"],
                    "Declared payload/allocation",
                    physical - window[0],
                )
            return "dram", window, physical
        window = (stack_pointer - below, stack_pointer + STACK_ABOVE)
        if self._memory_class(
            core, window[0], window[1] - window[0]
        ) == "tcm" and self._mapped_rw(core, *window):
            return "tcm", window, stack_pointer
        raise ValueError(
            "Driver stack needs aligned DRAM with room for arguments, or the calling core's mapped TCM"
        )

    def _parse_registers(
        self,
        registers: dict[str, int] | None,
        stack_pointer: int | None,
        lr: int | None,
    ) -> dict[str, int]:
        parsed = {}
        if registers is None:
            return parsed
        if not isinstance(registers, dict):
            raise ValueError("registers is {name: value}")
        for name, value in registers.items():
            if name == "pc":
                raise ValueError(
                    "registers cannot set pc; the call entry is the program counter"
                )
            if name not in NAMES and name not in regs.BANKED:
                raise ValueError("Unknown register: " + str(name))
            parsed[name] = _word(value, "Register " + name)
        if (
            "sp" in parsed
            and stack_pointer is not None
            and parsed["sp"] != stack_pointer
        ):
            raise ValueError("registers['sp'] and stack_pointer disagree")
        if "lr" in parsed and lr is not None and parsed["lr"] != lr:
            raise ValueError("registers['lr'] and lr disagree")
        return parsed

    def call(
        self,
        entry: Target,
        args: Sequence[int] = (),
        *,
        max_instructions: int = 10000,
        stack_pointer: int | None = None,
        cpsr: int = 0xD3,
        record_stores: bool = False,
        allowed_stores: Iterable[Range] | None = None,
        stack_bytes: int = STACK_BELOW,
        registers: dict[str, int] | None = None,
        lr: int | None = None,
        cpu: int | None = None,
        align_sp: bool = True,
        coverage: bool = False,
        allowed_code: Iterable[Range] | None = None,
        allowed_functions: Iterable[Target] | None = None,
        include_stack: bool = False,
        inject_interrupts: bool = True,
        deliver_faults: bool = False,
        cpu_identity: dict[int, int] | None = None,
        count_ranges: Iterable[Range] | None = None,
        trace: int = 0,
        store_limit: int = STORE_LIMIT,
    ) -> dict[str, object]:
        """Run a whole ARM routine to its real LR return or a strict failure.

        Arguments use r0-r3 and then the ordinary caller stack. Interrupts are
        masked by default. Enabling them requires explicitly prepared vectors,
        kernel state and guest controller configuration. A stopped/failing
        harness must be replaced; failures are never cleared automatically.

        Registered breakpoints apply to this call. ``record_stores`` logs ordered
        stores ``(address, width, value, pc)`` outside the caller stack window
        [SP-stack_bytes, SP+64): ordinary stores and successful STREX stores
        in one order, DRAM/alias stores by physical address. ``store_records``
        carries the same entries as dicts that add the guest ``virtual``
        address (physical vs declared alias evidence), ``exclusive`` and
        ``stack`` flags. ``include_stack=True`` also logs stack-window stores
        (``stack`` True) so exact stack write ranges can be asserted.
        ``allowed_stores`` is a tuple of half-open ``(lo, hi)`` ranges.
        ``store_limit`` bounds recorded stores (default 1048576); a changed
        bound appears in the result. A store outside ``allowed_stores``
        fails the call after the offending instruction (its
        effect is not prevented).

        ``registers={'r4': ..., 'sp': ..., 'lr': ..., 'cpsr': ..., 'spsr': ...,
        'sp_abt': ...}`` sets initial state after the usual setup (explicit
        values win; ``cpsr`` selects the entry mode, usr/fiq/irq/svc/abt/und/
        sys, and banked names reach the other modes' registers; ``pc`` is the
        entry). ``cpu=0|1`` runs this call on that core of the same board
        (shared DRAM/SRAM, private TCM, per-core CP14 identity); the other core
        stays non-runnable. ``stack_pointer`` may be DRAM or the core's TCM;
        4-byte alignment is always required, 8-byte only with ``align_sp``
        (disable it for mid-routine entry). ``lr=addr`` ends the call when PC
        reaches ``addr`` (before that instruction executes, so it is not
        counted; the call counts as returned) instead of the declared return address. The
        address may be inside a chosen image: control reaching it by any path
        ends the call, exactly like ``stop_at`` but reported as a return.

        ``coverage=True`` returns ``executed`` (sorted unique PCs of counted
        instructions, collected natively).
        ``count_ranges=((lo, hi), ...)`` (half-open, at most 256, either DRAM
        view) returns ``range_instructions``: a list, one int per range in the
        order given, of *executed* instructions whose address lies in the range
        (ranges may overlap; an instruction counts in each). It is collected
        natively on the same path as ``instructions``: a stub, stop, ``lr=`` or
        redirecting-observer hit whose instruction does not execute is not
        counted, whereas an observer that lets the instruction run is. The
        fetch guard's ``accepted_fetches``/``range_fetches`` are different:
        they count fetch *attempts* accepted by the guard, including
        breakpoint addresses (stubs, stops), and are not instruction counts.
        ``trace=N`` (0..1048576) returns ``trace``: the first N executed PCs in
        execution order (repeats kept, addresses as fetched so an alias
        fetch shows its alias address) and ``trace_truncated``; ``executed``
        stays sorted and unique. Without these options nothing extra runs.
        The result's ``registers`` are r0-r12, sp, lr, pc and cpsr of the
        running core when the call ended; ``call_index`` is the call's 0-based
        sequence number on this harness. ``allowed_code=((lo, hi), ...)`` /
        ``allowed_functions=(names or entries)`` narrow the fetch guard (which
        they enable for this call) to those ranges plus payloads and stub/stop
        addresses. ``cpu_identity={core: value}`` overrides the CP14 identity
        for this call. ``inject_interrupts=False`` never injects a pending IRQ
        (an injection is also skipped when PC is at an armed ``stop_at``
        address other than the call entry, so that stop fires first; a stop
        or ``lr=`` address equal to the entry is not armed until execution has
        left it). ``deliver_faults=True`` routes a native MPU
        prefetch/data permission abort or disabled-VFP undefined through the board's
        exception entry and continues in the stock vector (recorded in
        ``delivered_faults``); the vector word must be fetchable.
        """
        entry = self._entry(entry)
        args = tuple(args)
        if len(args) > 16 or any(
            type(arg) is not int or not -(1 << 31) <= arg < 1 << 32 for arg in args
        ):
            raise ValueError("Driver arguments must be at most 16 machine words")
        if type(max_instructions) is not int or max_instructions < 1:
            raise ValueError("Driver instruction budget must be positive")
        if type(store_limit) is not int or not 1 <= store_limit <= 1 << 24:
            raise ValueError("Store bound must be an integer in 1..16777216")
        core = self._core_arg(cpu)
        parsed = self._parse_registers(registers, stack_pointer, lr)
        if lr is not None and (type(lr) is not int or not 0 <= lr < WORD or lr & 3):
            raise ValueError("lr must be an aligned 32-bit ARM address")
        return_address = self._return_address if lr is None else lr
        initial_lr = parsed.get("lr", self._return_address if lr is None else lr)
        if "cpsr" in parsed:
            if cpsr != 0xD3 and cpsr != parsed["cpsr"]:
                raise ValueError("registers['cpsr'] and cpsr disagree")
            cpsr = parsed["cpsr"]
        if (
            type(cpsr) is not int
            or not 0 <= cpsr <= 0xFFFFFFFF
            or cpsr & 0x20
            or cpsr & 31 not in CALL_MODES
        ):
            raise ValueError(
                "Driver caller must use an ARM user, FIQ, IRQ, supervisor, abort, undefined or system mode"
            )
        if cpsr & 31 in (0x10, 0x1F) and "spsr" in parsed:
            raise ValueError("User and system modes have no SPSR")
        sp = parsed.get(
            "sp", self._stack_pointer if stack_pointer is None else stack_pointer
        )
        if type(sp) is not int:
            raise ValueError("stack_pointer must be an integer")
        if (
            type(stack_bytes) is not int
            or not 0x100 <= stack_bytes <= 0x100000
            or stack_bytes & 7
        ):
            raise ValueError(
                "Driver stack window must be an aligned size of 256 B..1 MiB"
            )
        if (
            type(align_sp) is not bool
            or type(coverage) is not bool
            or type(include_stack) is not bool
        ):
            raise ValueError("align_sp, coverage and include_stack must be bools")
        if type(inject_interrupts) is not bool or type(deliver_faults) is not bool:
            raise ValueError("inject_interrupts and deliver_faults must be bools")
        if type(record_stores) is not bool:
            raise ValueError("record_stores must be a bool")
        if include_stack and not record_stores:
            raise ValueError("include_stack requires record_stores")
        if type(trace) is not int or not 0 <= trace <= 1 << 20:
            raise ValueError("trace must be an integer 0..1048576")
        probe_ranges = None
        if count_ranges is not None:
            probe_ranges = []
            for item in count_ranges:
                try:
                    lo, hi = item
                except TypeError, ValueError:
                    raise ValueError("count_ranges needs (lo, hi) ranges") from None
                if (
                    type(lo) is not int
                    or type(hi) is not int
                    or not 0 <= lo < hi <= WORD
                ):
                    raise ValueError(
                        "count_ranges needs half-open 32-bit (lo, hi) ranges"
                    )
                probe_ranges.append((
                    self._memory.physical(lo),
                    self._memory.physical(hi - 1) + 1,
                ))
            if len(probe_ranges) > 256:
                raise ValueError("count_ranges accepts at most 256 ranges")
        space, window, physical_sp = self._plan_stack(core, sp, stack_bytes, align_sp)
        board, cpu_obj = self.board, self.board.cpus[core]
        machine = board.native
        if board.failure:
            raise ValueError("Cannot resume a failed isolated driver")
        if allowed_stores is not None:
            allowed_stores = [tuple(item) for item in allowed_stores]
            if any(
                len(item) != 2
                or any(type(v) is not int for v in item)
                or item[0] >= item[1]
                for item in allowed_stores
            ):
                raise ValueError(
                    "allowed_stores needs half-open (lo, hi) address ranges"
                )
            allowed_stores = [
                (self._memory.physical(lo), self._memory.physical(hi - 1) + 1)
                for lo, hi in allowed_stores
            ]
        identity = dict(enumerate(self._base_identity))
        identity.update(self._identity_map(cpu_identity))
        restricted = None
        if allowed_code is not None or allowed_functions is not None:
            restricted = []
            for item in allowed_code or ():
                try:
                    lo, hi = item
                except TypeError, ValueError:
                    raise ValueError("allowed_code needs (lo, hi) ranges") from None
                if (
                    type(lo) is not int
                    or type(hi) is not int
                    or not 0 <= lo < hi <= WORD
                ):
                    raise ValueError(
                        "allowed_code needs half-open 32-bit (lo, hi) ranges"
                    )
                restricted.append((
                    self._memory.physical(lo),
                    self._memory.physical(hi - 1) + 1,
                ))
            restricted.extend(self._function_ranges(allowed_functions or ()))
            if not restricted:
                raise ValueError("allowed_code/allowed_functions named no ranges")
        use_guard = self.fetch_guard or restricted is not None
        if args[4:]:
            data = b"".join(
                (arg & 0xFFFFFFFF).to_bytes(4, "little") for arg in args[4:]
            )
            if space == "dram":
                self._load_ram(((physical_sp, data),), False)
            else:
                board.write_memory(sp, data, core)
        self._select_core(core)
        cpu_obj.reg_write(CPSR, cpsr)
        for name, value in parsed.items():
            if name in regs.BANKED:
                regs.write_register(cpu_obj, name, value)
        cpu_obj.reg_write(SP, sp)
        cpu_obj.reg_write(LR, initial_lr)
        cpu_obj.reg_write(PC, entry)
        for index, register in enumerate(regs.GENERAL[:4]):
            cpu_obj.reg_write(
                register, args[index] & 0xFFFFFFFF if index < len(args) else 0
            )
        for name, value in parsed.items():
            if name not in regs.BANKED and name not in ("cpsr", "sp", "lr"):
                cpu_obj.reg_write(NAMES[name], value)
        start, ticks = board.instructions, board.virtual_ticks
        machine.driver_select_core(core)
        self._hits, self._stopped_at, self._stop_info, self._hit_limit = (
            [],
            None,
            None,
            max_instructions,
        )
        call_index, self._calls = self._calls, self._calls + 1
        track = coverage or bool(self.data_overrides)
        guard_ranges: list[Range] | None = None
        vectors: list[Range] = []
        opcode = 0xE59FF018
        if use_guard:
            guard_ranges = self._code_ranges()
            if restricted is not None:
                payload_ranges = [
                    (p["address"], p["address"] + p["length"]) for p in self.payloads
                ]
                guard_ranges = _merge(
                    _intersect(
                        restricted,
                        [(base, base + len(data)) for base, data in self.images],
                    )
                    + payload_ranges
                    + [
                        (a, a + 4)
                        for a, item in self._breakpoints.items()
                        if item["kind"] in ("stub", "stop", "substitute")
                    ]
                )
            guard_ranges = _subtract(
                guard_ranges,
                [
                    (o["address"], o["address"] + o["length"])
                    for o in self.data_overrides
                ],
            )
            vectors, opcode = self._guard_configuration(
                cpu_obj, core, guard_ranges, deliver_faults
            )
        unarmed = (
            [entry]
            if entry == return_address
            or self._breakpoints.get(entry, {}).get("kind") == "stop"
            else []
        )
        options = dict(
            core=core,
            inject_interrupts=inject_interrupts,
            deliver_faults=deliver_faults,
            memory=self._observation_memory,
            return_address=return_address,
            breakpoints=sorted(self._breakpoints),
            unarmed=unarmed,
            stop_addresses=[
                a for a, item in self._breakpoints.items() if item["kind"] == "stop"
            ],
            ranges=guard_ranges,
            vectors=vectors,
            vector_opcode=opcode,
            stack=window,
            allowed_stores=allowed_stores,
            record_stores=record_stores,
            include_stack=include_stack,
            store_limit=store_limit,
            coverage=track,
            count_ranges=probe_ranges or [],
            trace=trace,
        )
        faults: list[dict[str, object]] = []
        observation: dict[str, object] | None = None
        skip_once: int | None = None
        fetch_failure: dict[str, object] | None = None
        try:
            for index, value in identity.items():
                machine.rprofile_identity(index, value)
            machine.driver_begin(json.dumps(options), board.transaction_limit)
            while not board.failure and self._stopped_at is None:
                remaining = max_instructions - (board.instructions - start)
                if remaining <= 0:
                    # A routine whose final instruction used the exact budget
                    # still returns when its resulting PC is the declared LR.
                    if cpu_obj.reg_read(PC) != return_address:
                        board._stop(
                            cpu_obj, core, "Isolated driver instruction bound reached"
                        )
                    break
                machine.driver_resume(skip_once)
                skip_once = None
                before = board.instructions
                board.executing_cpu = core
                try:
                    machine.run(remaining, board.quantum, None, None)
                except Exception as error:
                    pc = cpu_obj.reg_read(PC)
                    if (
                        use_guard
                        and guard_ranges is not None
                        and not any(
                            lo <= self._memory.physical(pc) < hi
                            for lo, hi in guard_ranges
                        )
                    ):
                        fetch_failure = dict(
                            kind="blocked",
                            reason="instruction fetch outside declared code ranges",
                            pc=pc,
                            size=4,
                            opcode=None,
                            registers=[cpu_obj.reg_read(r) for r in regs.GENERAL[:4]],
                        )
                        board._stop(
                            cpu_obj,
                            core,
                            f"Isolated driver fetch guard: instruction fetch outside declared code ranges at {pc:#x}",
                        )
                    else:
                        details = {}
                        sctlr = cpu_obj.cpr_read(15, 0, 1, 0, 0, 0, False)
                        if not sctlr & 1 and pc >= 0x80000000:
                            details["hint"] = (
                                "MPU disabled: the default PMSAv7 map makes addresses >= 0x80000000 execute-never; call enable_flat_mpu() or program an MPU region"
                            )
                        board._stop(cpu_obj, core, str(error), **details)
                finally:
                    board.executing_cpu = None
                observation = _json_object(json.loads(machine.driver_state()))
                event = observation["event"]
                if board.failure:
                    break
                if event is None:
                    if cpu_obj.reg_read(PC) == return_address:
                        break
                    state = board.state()
                    waiting = state["waiting"]
                    if not isinstance(waiting, list) or core >= len(waiting):
                        raise RuntimeError("Board waiting state must be a core list")
                    if not isinstance(waiting[core], bool):
                        raise RuntimeError("Board waiting state must contain booleans")
                    if waiting[core]:
                        board._stop(
                            cpu_obj,
                            core,
                            "Isolated driver entered WFI; no RTOS caller continuation supplied",
                        )
                    elif board.instructions == before:
                        board._stop(
                            cpu_obj,
                            core,
                            "Isolated driver made no instruction progress",
                        )
                    continue
                event = _json_object(event)
                event_kind = _text(event["kind"])
                if event_kind == "return":
                    break
                if event_kind == "failure":
                    details: dict[str, object] = {}
                    message = _text(event["message"])
                    fetch_guard = event.get("fetch_guard")
                    if fetch_guard is not None:
                        fetch_guard = _json_object(fetch_guard)
                        details["fetch_guard"] = fetch_guard
                        message += f" at {_integer(fetch_guard['pc']):#x}"
                    if message == "Native synchronous fault in isolated driver":
                        sctlr = cpu_obj.cpr_read(15, 0, 1, 0, 0, 0, False)
                        if not sctlr & 1:
                            details["hint"] = (
                                "MPU disabled: the default PMSAv7 map makes addresses >= 0x80000000 execute-never; call enable_flat_mpu() or program an MPU region"
                            )
                    board._stop(cpu_obj, core, message, **details)
                    break
                pc = _integer(event["pc"])
                self._breakpoint(cpu_obj, core, pc)
                # An observer can resume the same instruction once. A stub or
                # instruction substitution redirects without executing it.
                skip_once = pc if cpu_obj.reg_read(PC) == pc else None
            observation = _json_object(json.loads(machine.driver_state()))
        finally:
            machine.driver_end()
            for index, value in enumerate(self._base_identity):
                machine.rprofile_identity(index, value)
        bus = _json_object(observation["bus"])
        board.transactions = _json_objects(bus["transactions"])
        omitted = _integer(bus["omitted"])
        if omitted and not board.failure:
            board._stop(cpu_obj, core, "Isolated driver transaction bound reached")
        faults = (
            [
                dict(
                    fault,
                    fault_pc=hex(int(_text(fault["fault_pc"]), 16) & 0xFFFFFFFF),
                )
                for fault in _json_objects(observation["faults"])
            ]
            if deliver_faults
            else []
        )
        guard = _json_object(observation["guard"])
        if fetch_failure is not None:
            guard = dict(
                guard,
                halted=True,
                halt=fetch_failure,
                checked_fetches=_integer(guard["checked_fetches"]) + 1,
            )
        if record_stores:
            record_rows = _json_objects(observation["records"])
            records: list[dict[str, object]] | None = record_rows
            stores: list[tuple[int, int, int, int]] | None = [
                (
                    _integer(record["physical"]),
                    _integer(record["width"]),
                    _integer(record["value"]),
                    _integer(record["pc"]),
                )
                for record in record_rows
            ]
        else:
            records = None
            stores = None
        executed = _integers(observation["executed"]) if track else None
        if self.data_overrides and executed is not None:
            for pc in executed:
                override = next(
                    (
                        o
                        for o in self.data_overrides
                        if self._memory.physical(pc) < o["address"] + o["length"]
                        and o["address"] < self._memory.physical(pc) + 4
                    ),
                    None,
                )
                if override is not None:
                    board._stop(
                        cpu_obj,
                        core,
                        "Executed instruction fetched from a data_override byte",
                        fetch_pc=hex(pc),
                        override_address=hex(override["address"]),
                        override_reason=override["reason"],
                    )
                    break
        range_instructions = (
            _integers(observation["counts"]) if probe_ranges is not None else None
        )
        trace_pcs = _integers(observation["trace"]) if trace else None
        trace_truncated = (
            _integer(observation["trace_total"]) > len(trace_pcs)
            if trace_pcs is not None
            else None
        )
        final = {name: cpu_obj.reg_read(NAMES[name]) for name in FINAL_REGISTERS}
        returned = (
            not board.failure
            and self._stopped_at is None
            and final["pc"] == return_address
        )
        stubbed = sum(item["kind"] in ("stub", "observe_return") for item in self._hits)
        limits = "Isolated declared caller/model state; no full boot, physical timing or hardware success established"
        if self._breakpoints:
            counts = [
                sum(r["kind"] == k for r in self._breakpoints.values())
                for k in ("stub", "observe", "stop", "substitute")
            ]
            limits += f"; {counts[0]} callees substituted ({stubbed} stub returns taken), {counts[1]} observers, {counts[2]} stop points"
            if counts[3]:
                replaced = sum(item["kind"] == "substitute" for item in self._hits)
                limits += f", {counts[3]} instructions substituted ({replaced} substitutions taken)"
        if self.patches:
            limits += f"; {len(self.patches)} mapped firmware byte patches"
        if self.data_overrides:
            limits += f"; {len(self.data_overrides)} runtime data overrides inside mapped images"
        if self.payloads:
            limits += f"; {len(self.payloads)} DRAM payload images"
        if restricted is not None and guard_ranges is not None:
            limits += f"; fetch restricted to {len(guard_ranges)} declared code ranges"
        if identity != dict(enumerate(self._default_identity)):
            limits += "; CP14 CPU identity overridden"
        if deliver_faults:
            limits += f"; {len(faults)} native faults delivered through the board's exception entry"
        irq_trace = _json_objects(bus["irq_events"]) + _json_objects(
            observation["irq_events"]
        )
        result: dict[str, object] = {}
        result.update(
            entry=entry,
            cpu=core,
            returned=returned,
            return_address=return_address,
            return_value=final["r0"] if returned else None,
            registers=final,
            call_index=call_index,
            instructions=board.instructions - start,
            virtual_ticks=board.virtual_ticks - ticks,
            failure=board.failure,
            transactions=list(board.transactions),
            irq_events=irq_trace,
            irq_requests=[
                item for item in irq_trace if item.get("name") in IRQ_REQUEST_RECORDS
            ],
            pending_scheduled_completions=self._pending_completions(),
            breakpoints=list(self._hits),
            stopped_at=self._stopped_at,
            stop=dict(self._stop_info) if self._stop_info is not None else None,
            stores=stores,
            store_records=records,
            executed=executed if coverage else None,
            range_instructions=range_instructions,
            trace=trace_pcs,
            trace_truncated=trace_truncated,
            delivered_faults=faults,
            cpu_identity=identity,
            patches=[dict(item) for item in self.patches],
            data_overrides=[dict(item) for item in self.data_overrides],
            data_override_events=[dict(item) for item in self.data_override_events],
            payloads=[dict(item) for item in self.payloads],
            payload_replacements=[dict(item) for item in self.payload_replacements],
            allocations=[dict(item) for item in self.allocations],
            allowed_code=(
                None
                if restricted is None or guard_ranges is None
                else list(guard_ranges)
            ),
            fetch_guard=guard if use_guard else dict(enabled=False),
            runtime="shared",
            device_graph=json.loads(json.dumps(board.device_graph)),
            observed_memory=json.loads(json.dumps(self._observation_memory)),
            scene=json.loads(json.dumps(board.input_manifest.get("scene", {}))),
            assets=json.loads(json.dumps(board.input_manifest.get("assets", {}))),
            transaction_records_omitted=omitted,
            limits=limits,
        )
        if self._profile_identity is not None:
            result["profile"] = dict(self._profile_identity)
        if store_limit != STORE_LIMIT:
            result["store_limit"] = store_limit
            result["limits"] = limits + f"; explicit store bound {store_limit}"
        return result
