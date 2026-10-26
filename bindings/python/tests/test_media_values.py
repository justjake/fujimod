"""The media interface works without test endpoints, including production wheels."""

from media_contract import check_media_values


def test_media_values() -> None:
    assert check_media_values()
