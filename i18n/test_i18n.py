from pathlib import Path

import pytest

from i18n import check, extract


def catalog(tmp_path: Path, entries: str) -> Path:
    path = tmp_path / "translations"
    path.mkdir()
    (path / "fujimod.pot").write_text(
        'msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n\n' + entries)
    return path


def test_catalogs_allow_translated_and_empty_entries(tmp_path: Path) -> None:
    directory = catalog(tmp_path, 'msgctxt "menu.label"\nmsgid "OFF"\nmsgstr ""\n')
    (directory / "de.po").write_text(
        'msgctxt "menu.label"\nmsgid "OFF"\nmsgstr "AUS"\n')
    assert check.check(directory) == {directory / "de.po": 1}


def test_catalog_rejects_unknown_keys_and_locales(tmp_path: Path) -> None:
    directory = catalog(tmp_path, 'msgctxt "menu.label"\nmsgid "OFF"\nmsgstr ""\n')
    (directory / "de.po").write_text(
        'msgctxt "menu.label"\nmsgid "ON"\nmsgstr "AN"\n')
    with pytest.raises(check.CatalogError, match="unknown entries"):
        check.check(directory)
    (directory / "de.po").write_text(
        'msgctxt "menu.label"\nmsgid "OFF"\nmsgstr "AUS"\n')
    (directory / "xx.po").write_text(
        'msgctxt "menu.label"\nmsgid "OFF"\nmsgstr ""\n')
    with pytest.raises(check.CatalogError, match="not a camera language"):
        check.check(directory)


def test_extractor_finds_marked_declarations(tmp_path: Path) -> None:
    source = tmp_path / "menu.c"
    source.write_text('X70_MENU_ITEM(row) = {.label = X70_I18N("FILM BORDERS")};\n')
    assert extract.marked_strings([source]) == {"FILM BORDERS"}
