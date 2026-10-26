"""Create external card inputs through the Rust storage library."""

import hashlib
import json
from collections.abc import Iterable
from pathlib import Path

from fujimod._native import emulator as _native


def format_card(
    files: Iterable[tuple[str, bytes]] = (),
    *,
    sectors: int = 0x800000,
    partition_start: int = 8192,
    sectors_per_cluster: int = 64,
    label: str = "CAMERA",
    read_only: Iterable[str] = (),
    max_dirty_sectors: int = 1 << 21,
) -> _native.CardMedium:
    options = json.dumps(
        dict(
            sectors=sectors,
            max_dirty_sectors=max_dirty_sectors,
            partition_start=partition_start,
            sectors_per_cluster=sectors_per_cluster,
            label=label,
            read_only_files=list(read_only),
        )
    )
    return _native.CardMedium.formatted(options, list(files))


def load_card_image(
    path: str | Path,
    *,
    sectors: int = 0x1000000,
    max_dirty_sectors: int = 1 << 21,
) -> tuple[_native.CardMedium, dict[str, int | str]]:
    """Copy a bounded external raw image into a new, detached Rust medium."""
    path = Path(path)
    size = path.stat().st_size
    if not size or size % 512 or size > sectors * 512:
        raise ValueError("Raw card image must contain whole sectors within capacity")
    card = _native.CardMedium.blank(
        json.dumps(dict(sectors=sectors, max_dirty_sectors=max_dirty_sectors))
    )
    digest = hashlib.sha256()
    count = 0
    offset = 0
    with path.open("rb") as source:
        while block := source.read(1 << 20):
            digest.update(block)
            if any(block):
                card.write_sectors(offset // 512, block)
                count += sum(any(block[i : i + 512]) for i in range(0, len(block), 512))
            offset += len(block)
    if offset != size:
        raise ValueError("Raw card image changed during import")
    return card, dict(
        path=str(path),
        bytes=size,
        sectors=size // 512,
        nonzero_sectors=count,
        sha256=digest.hexdigest(),
    )
