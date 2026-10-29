import builtins
from typing import TypedDict

class TokenCounts(TypedDict):
    literals: int
    matches: int

class Compression(TypedDict):
    packed_size: int
    unpacked_size: int
    page_size: int
    page_count: int
    method: int
    token_counts: TokenCounts

class Stream(TypedDict, total=False):
    name: str
    dat_offset: str
    packed_size: int
    decoded_size: int
    sha256: str
    header_word_bits: int
    base: str
    initialized_size: str
    executable: bool

def invert(data: bytes) -> bytes: ...
def inner_header(data: bytes) -> tuple[int, int, int] | None: ...
def decompress(data: bytes) -> tuple[bytes, Compression]: ...
def recompress(source: bytes, replacement: bytes) -> bytes: ...
def scan(source: bytes) -> list[Stream]: ...
def raw(
    data: bytes,
    offset: int,
    size: int,
    base: int,
    digest: str,
    architecture_bits: int,
    header_bits: int | None = ...,
    entry: int | None = ...,
) -> bytes: ...
def decode(
    source: bytes, selections: list[tuple[str, int, int, int, bool, bool]]
) -> Image: ...
def outer_header(data: bytes) -> dict[str, object]: ...

class Unit:
    @property
    def name(self) -> str: ...
    @property
    def base(self) -> int: ...
    @property
    def executable(self) -> bool: ...
    @property
    def bytes(self) -> builtins.bytes: ...

class Image:
    @property
    def source_sha256(self) -> str: ...
    @property
    def units(self) -> list[Unit]: ...

class Overlay(TypedDict):
    name: str
    logical_id: int
    semaphore: int
    address: int
    size: int
    dat_offset: int
    sha256: str

def overlays(
    data: bytes,
    offset: int,
    base: int,
    size: int,
    digest: str,
    alias: int,
    bias: int,
    section_id: int,
    streams: list[tuple[int, int]],
) -> list[Overlay]: ...
