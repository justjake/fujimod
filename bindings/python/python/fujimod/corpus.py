"""Function-family index. Inputs are boundaries and mapped bytes from any source.

Function ids and family ids are local to one build; persist function keys in locators.
All addresses are integers and ranges are half-open. No firmware policy lives here.
"""

import json
from collections.abc import Iterable, Mapping
from os import PathLike
from typing import Self, cast

from ._corpus_types import (
    DataSymbol,
    DataWitness,
    Family,
    FunctionCandidate,
    FunctionDetail,
    FunctionKey,
    FunctionView,
    NameDonor,
    NameSupport,
    Pair,
    Reference,
    SampleGroup,
    Stats,
)
from ._native import corpus as _core

CorpusIndex = _core.CorpusIndex


def align_sequences(left: list[str], right: list[str]) -> list[tuple[int | None, int | None]]:
    """Return paired token indexes, retaining unmatched tokens on either side."""
    return _core.align_sequences(left, right)


class Index:
    def __init__(self) -> None:
        self._native = CorpusIndex()

    @classmethod
    def open(cls, path: str | PathLike[str]) -> Self:
        """Open a saved store. Lookups query it directly; graph queries such as
        candidates, pairs and name support, and any change, load it once."""
        obj = cls.__new__(cls)
        obj._native = CorpusIndex.open(str(path))
        return obj

    def save(self, path: str | PathLike[str]) -> None:
        self._native.save(str(path))

    def add_unit(
        self, metadata: Mapping[str, object], regions: Iterable[tuple[int, bytes]]
    ) -> None:
        self._native.add_unit(json.dumps(metadata), list(regions))

    def build(self) -> None:
        self._native.build()

    def merge(self, other: Self) -> None:
        self._native.merge(other._native)

    def subset(self, firmwares: Iterable[str]) -> Self:
        obj = self.__class__.__new__(self.__class__)
        obj._native = self._native.subset(list(firmwares))
        return obj

    def _query(self, operation: str, **arguments: object) -> object:
        return json.loads(self._native.query(operation, json.dumps(arguments)))

    def stats(self) -> Stats:
        return cast(Stats, self._query("stats"))

    def find(
        self,
        name: str | None = None,
        *,
        address: int | None = None,
        firmware: str | None = None,
        unit: str | None = None,
    ) -> list[FunctionView]:
        return cast(
            list[FunctionView],
            self._query(
                "find", name=name, address=address, firmware=firmware, unit=unit
            ),
        )

    def family(self, family: int) -> Family | None:
        return cast(Family | None, self._query("family", id=family))

    def members(self, family: int, firmware: str | None = None) -> list[FunctionView]:
        return cast(
            list[FunctionView], self._query("members", id=family, firmware=firmware)
        )

    def candidates(self, function: int, firmware: str) -> list[FunctionCandidate]:
        """Analysis nominations; shape-only matches keep weak grade and all ambiguities."""
        return cast(
            list[FunctionCandidate],
            self._query("candidates", id=function, firmware=firmware),
        )

    def string_frequencies(self, strings: Iterable[str]) -> dict[str, dict[str, int]]:
        return cast(
            dict[str, dict[str, int]],
            self._query("string_frequencies", strings=list(strings)),
        )

    def body_sha256(self, function: int) -> str | None:
        return cast(str | None, self._query("body_sha256", id=function))

    def name_support(self, function: int, other: int) -> NameSupport:
        return cast(NameSupport, self._query("name_support", id=function, other=other))

    def find_strings(
        self, strings: Iterable[str], firmware: str | None = None
    ) -> list[FunctionView]:
        return cast(
            list[FunctionView],
            self._query("find_strings", strings=list(strings), firmware=firmware),
        )

    def add_name(self, function: int, name: str, source: str) -> None:
        self._native.add_name(function, name, source)

    def read(self, firmware: str, unit: str, address: int, size: int) -> bytes | None:
        result = self._query(
            "read", firmware=firmware, unit=unit, address=address, size=size
        )
        return None if result is None else bytes(cast(list[int], result))

    def add_symbol(self, symbol: DataSymbol) -> None:
        self._native.add_symbol(json.dumps(symbol))

    def symbols(
        self,
        name: str | None = None,
        *,
        firmware: str | None = None,
        unit: str | None = None,
    ) -> list[DataSymbol]:
        return cast(
            list[DataSymbol],
            self._query("symbols", name=name, firmware=firmware, unit=unit),
        )

    def features(self, function: int) -> FunctionDetail | None:
        return cast(FunctionDetail | None, self._query("features", id=function))

    def sample_links(self, count: int = 50, seed: int = 0) -> dict[str, SampleGroup]:
        return cast(
            dict[str, SampleGroup], self._query("sample_links", count=count, seed=seed)
        )

    def data_witnesses(self, function: int, length: int = 64) -> list[DataWitness]:
        return cast(
            list[DataWitness], self._query("data_witnesses", id=function, length=length)
        )

    def normalized_tokens(self, function: int) -> list[str]:
        return cast(list[str], self._query("normalized_tokens", id=function))

    def calls(self, function: int) -> list[list[int | FunctionView | None]]:
        return cast(
            list[list[int | FunctionView | None]], self._query("calls", id=function)
        )

    def data_references(self, function: int) -> list[Reference]:
        return cast(list[Reference], self._query("data_references", id=function))

    def references_to(
        self, firmware: str, address: int, unit: str | None = None
    ) -> list[list[FunctionView | Reference]]:
        return cast(
            list[list[FunctionView | Reference]],
            self._query("references_to", firmware=firmware, address=address, unit=unit),
        )

    def search(
        self,
        firmware: str,
        pattern: bytes,
        *,
        unit: str | None = None,
        mask: bytes | None = None,
    ) -> list[FunctionKey]:
        return cast(
            list[FunctionKey],
            self._query(
                "search",
                firmware=firmware,
                unit=unit,
                pattern=list(pattern),
                mask=list(mask) if mask is not None else None,
            ),
        )

    def pairs(self, reference: str, other: str) -> list[Pair]:
        return cast(list[Pair], self._query("pairs", reference=reference, other=other))

    def pairs_for(self, function: int, other: str) -> list[Pair]:
        """Return pair rows for one reference function without querying all families."""
        return cast(
            list[Pair], self._query("pairs_for", id=function, other=other)
        )

    def names(
        self,
        function: int,
        donors: list[str] | None = None,
        *,
        normalized_only: bool = False,
    ) -> dict[str, list[NameDonor]]:
        return cast(
            dict[str, list[NameDonor]],
            self._query(
                "names", id=function, donors=donors, normalized_only=normalized_only
            ),
        )

    def annotate(self, family: int, kind: str, text: str, source: str) -> None:
        self._native.annotate(
            family, json.dumps(dict(kind=kind, text=text, source=source))
        )
