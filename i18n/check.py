"""Validate FujiMod's public Weblate catalogs."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys


HERE = Path(__file__).resolve().parent

# Stock camera language ordinals, in menu order. Weblate's locale names map
# to these values; the generated C table uses the same map.
LANGUAGES: dict[str, int] = json.loads((HERE / "languages.json").read_text(encoding="utf-8"))


class CatalogError(ValueError):
    pass


def unquote(value: str) -> str:
    from ast import literal_eval
    try:
        result = literal_eval(value)
    except (SyntaxError, ValueError) as error:
        raise CatalogError(f"invalid PO string {value!r}") from error
    if not isinstance(result, str):
        raise CatalogError(f"invalid PO string {value!r}")
    return result


def parse_po(path: Path) -> dict[tuple[str, str], str]:
    entries: dict[tuple[str, str], str] = {}
    fields: dict[str, str] = {}
    current: str | None = None

    def finish() -> None:
        nonlocal fields
        msgid = fields.get("msgid")
        if msgid:
            context = fields.get("msgctxt")
            if not context:
                raise CatalogError(f"{path}: entry {msgid!r} has no msgctxt")
            identity = (context, msgid)
            if identity in entries:
                raise CatalogError(f"{path}: duplicate entry {identity!r}")
            entries[identity] = fields.get("msgstr", "")
        fields = {}

    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line:
            finish()
            current = None
        elif line.startswith("#"):
            continue
        elif line.split(" ", 1)[0] in ("msgctxt", "msgid", "msgstr"):
            current, value = line.split(" ", 1)
            if not value.startswith('"'):
                raise CatalogError(f"{path}:{number}: {current} needs a quoted string")
            fields[current] = unquote(value)
        elif line.startswith('"'):
            if current is None:
                raise CatalogError(f"{path}:{number}: continuation outside an entry")
            fields[current] += unquote(line)
        else:
            raise CatalogError(f"{path}:{number}: unsupported PO syntax {line!r}")
    finish()
    return entries


def check(directory: Path = HERE) -> dict[Path, int]:
    """Validate one catalog directory and return its translated entry counts."""
    sources = parse_po(directory / "fujimod.pot")
    if any(sources.values()):
        raise CatalogError("fujimod.pot must contain empty msgstr values")
    results: dict[Path, int] = {}
    for path in sorted(directory.glob("*.po")):
        locale = path.stem.replace("-", "_")
        if locale not in LANGUAGES:
            raise CatalogError(f"{path}: locale {locale!r} is not a camera language")
        entries = parse_po(path)
        unknown = set(entries) - set(sources)
        if unknown:
            raise CatalogError(f"{path}: unknown entries: {sorted(unknown)!r}")
        results[path] = sum(bool(value) for value in entries.values())
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", nargs="?", type=Path, default=HERE)
    args = parser.parse_args()
    try:
        results = check(args.directory)
    except CatalogError as error:
        print(error, file=sys.stderr)
        return 1
    for path, translated in results.items():
        print(f"{path.name}: {translated} translated")
    print(f"{len(parse_po(args.directory / 'fujimod.pot'))} source strings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
