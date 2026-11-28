"""Extract and check the English source catalog used by translated builds.

Source extraction scans a tree of C declarations for X70_I18N markers; catalog
validation runs against the checked-in catalogs when --sources is omitted."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys

from i18n.check import HERE, CatalogError, check as check_catalog, parse_po


MARK = re.compile(r'X70_I18N\s*\(\s*"((?:[^"\\]|\\.)*)"\s*\)')


def marked_strings(paths: list[Path]) -> set[str]:
    found: set[str] = set()
    for path in paths:
        for match in MARK.finditer(path.read_text(encoding="utf-8")):
            found.add(bytes(match.group(1), "utf-8").decode("unicode_escape"))
    return found


def check_sources(sources: set[str], catalog: Path = HERE / "fujimod.pot") -> None:
    entries = parse_po(catalog)
    msgids = {key[1] for key in entries}
    missing = sources - msgids
    stale = msgids - sources
    if missing or stale:
        raise CatalogError(f"catalog mismatch: missing={sorted(missing)!r}, stale={sorted(stale)!r}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", nargs="+", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.sources:
        try:
            results = check_catalog(HERE)
        except CatalogError as error:
            print(error, file=sys.stderr)
            return 1
        print(f"{len(parse_po(HERE / 'fujimod.pot'))} source strings; "
              f"{len(results)} translation catalogs")
        return 0
    try:
        found = marked_strings(args.sources)
        check_sources(found)
    except CatalogError as error:
        print(error, file=sys.stderr)
        return 1
    print("\n".join(sorted(found)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
