"""JSON result shapes returned by the function index."""

from typing import NotRequired, TypedDict


class FunctionKey(TypedDict):
    firmware: str
    unit: str
    address: int


class Annotation(TypedDict):
    kind: str
    text: str
    source: str


class NameDonor(FunctionKey):
    grade: str
    sources: list[str]


class FunctionView(TypedDict):
    id: int
    family: int
    key: FunctionKey
    names: list[str]
    instructions: int
    signature: str
    name_donors: NotRequired[list[NameDonor]]


class Family(TypedDict):
    id: int
    members: list[int]
    annotations: list[Annotation]


class FunctionDetail(TypedDict):
    key: FunctionKey
    architecture: str
    names: list[str]
    annotations: list[Annotation]
    ranges: list[list[int]]
    tokens: list[int]
    signature: list[int]
    instructions: int
    calls: list[list[int]]
    references: list["Reference"]
    constants: list[int]
    strings: list[str]
    ngrams: list[int]


class Pair(TypedDict):
    reference: FunctionKey
    other: FunctionKey
    grade: str
    similarity: float


class FunctionCandidate(TypedDict):
    function: FunctionView
    grade: str
    origins: list[str]


class DataSymbol(TypedDict):
    firmware: str
    unit: str
    address: int
    length: int
    name: str
    kind: str
    source: str


class Reference(TypedDict):
    site: int
    target: int
    kind: str
    text: str | None


class NameSupport(TypedDict):
    corroborated: bool
    exact_body_bytes: bool
    shared_rare_strings: list[str]
    independent_callee_pairs: int
    constant_tables: list[str]


class LinkSample(TypedDict):
    left: FunctionView
    right: FunctionView
    pair: Pair


class SampleGroup(TypedDict):
    population: int
    links: list[LinkSample]


class DataWitness(TypedDict):
    address: int
    length: int
    sha256: str
    distinct_words: int


class Stats(TypedDict):
    functions: int
    families: int
    firmwares: dict[str, int]
