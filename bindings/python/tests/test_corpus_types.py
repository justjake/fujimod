"""Runtime checks for corpus JSON result annotations."""

from typing import get_type_hints, is_typeddict

from fujimod import _corpus_types


def test_corpus_typed_dict_annotations_resolve_after_module_import() -> None:
    types = [
        value
        for value in vars(_corpus_types).values()
        if isinstance(value, type) and is_typeddict(value)
    ]

    assert types
    for typed_dict in types:
        get_type_hints(typed_dict, vars(_corpus_types), vars(_corpus_types))
