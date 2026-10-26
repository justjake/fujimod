"""Explicit ARM caller RAM views and address canonicalization."""

from dataclasses import dataclass


@dataclass(frozen=True)
class MemoryRegion:
    begin: int
    end: int
    kind: str
    core: int | None = None

    def __post_init__(self) -> None:
        if (
            type(self.begin) is not int
            or type(self.end) is not int
            or not 0 <= self.begin < self.end <= 1 << 32
        ):
            raise ValueError("RAM region needs an ordered 32-bit address interval")
        if self.kind not in ("dram", "sram", "vectors", "tcm"):
            raise ValueError("Unknown caller RAM region kind")
        if self.core is not None and (type(self.core) is not int or self.core < 0):
            raise ValueError("RAM region core must be a nonnegative integer")


@dataclass(frozen=True)
class MemoryLayout:
    dram: tuple[int, int]
    aliases: tuple[tuple[int, int, int], ...]
    regions: tuple[MemoryRegion, ...]

    def __post_init__(self) -> None:
        begin, end = self.dram
        MemoryRegion(begin, end, "dram")
        spans = []
        for address, physical, size in self.aliases:
            if (
                any(type(value) is not int for value in (address, physical, size))
                or size < 1
                or not 0 <= address < address + size <= 1 << 32
                or not begin <= physical < physical + size <= end
            ):
                raise ValueError(
                    "RAM alias must map an ordered span into declared DRAM"
                )
            spans.append((address, address + size))
        spans.sort()
        if any(a[1] > b[0] for a, b in zip(spans, spans[1:], strict=False)):
            raise ValueError("RAM aliases overlap")
        for index, region in enumerate(self.regions):
            for other in self.regions[:index]:
                if (
                    region.core is None
                    or other.core is None
                    or region.core == other.core
                ) and max(region.begin, other.begin) < min(region.end, other.end):
                    raise ValueError("Caller RAM regions overlap on the same core")

    def physical(self, address: int) -> int:
        for base, physical, size in self.aliases:
            if base <= address < base + size:
                return physical + address - base
        return address

    def contains_dram(self, address: int, size: int) -> bool:
        physical = self.physical(address)
        return (
            type(size) is int
            and size >= 0
            and self.dram[0] <= physical <= physical + size <= self.dram[1]
            and (size == 0 or self.physical(address + size - 1) == physical + size - 1)
        )

    def classify(self, core: int, address: int, size: int) -> str | None:
        if (
            type(address) is not int
            or type(size) is not int
            or size < 1
            or not 0 <= address < address + size <= 1 << 32
        ):
            return None
        if self.contains_dram(address, size):
            return "dram"
        for region in self.regions:
            if (
                (region.core is None or region.core == core)
                and region.begin <= address
                and address + size <= region.end
            ):
                return region.kind
        return None
