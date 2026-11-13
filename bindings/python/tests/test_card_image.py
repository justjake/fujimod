"""A medium exported as a raw image reloads with the same sectors and file tree."""

import json

import pytest
from fujimod.emulator.media import format_card, load_card_image, write_card_image

SECTORS = 0x800000


def test_export_reload_round_trip(tmp_path) -> None:
    medium = format_card([("A/HELLO.TXT", b"hello")], sectors=SECTORS)
    medium.write_sectors(1 << 20, b"\x5a" * 1024)
    image = tmp_path / "card.img"
    report = write_card_image(medium, image)
    assert report["bytes"] == SECTORS * 512 == image.stat().st_size
    reloaded, loaded = load_card_image(image, sectors=SECTORS)
    assert reloaded.sparse_sectors() == medium.sparse_sectors()
    assert loaded["nonzero_sectors"] == report["nonzero_sectors"]
    assert reloaded.read("A/HELLO.TXT") == b"hello"
    assert json.loads(reloaded.tree()) == json.loads(medium.tree())


def test_export_never_replaces_an_existing_image(tmp_path) -> None:
    medium = format_card(sectors=SECTORS)
    image = tmp_path / "card.img"
    write_card_image(medium, image)
    with pytest.raises(FileExistsError):
        write_card_image(medium, image)
