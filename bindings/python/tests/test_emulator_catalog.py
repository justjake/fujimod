import json
from typing import cast, get_args, get_type_hints, is_typeddict

import pytest
from fujimod.emulator import _catalog_types, catalog


def test_reference_resolution_returns_a_typed_selection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    expected = {
        "soc": {"id": "exr2"},
        "body": {"id": "x70"},
        "firmware": {"id": "x70-110"},
        "parts": [],
    }
    calls = []

    def resolve(body_id: str, firmware_id: str) -> str:
        calls.append((body_id, firmware_id))
        return json.dumps(expected)

    monkeypatch.setattr(
        catalog._native_catalog, "resolve_reference", resolve, raising=False
    )

    result: catalog.ResolvedSelection = catalog.resolve_reference("x70", "x70-110")

    assert result == expected
    assert calls == [("x70", "x70-110")]


def test_firmware_resolution_uses_only_the_firmware_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    expected = {
        "soc": {"id": "exr2"},
        "body": {"id": "x70"},
        "firmware": {"id": "x70-110"},
        "parts": [],
    }
    calls = []

    def resolve(firmware_id: str) -> str:
        calls.append(firmware_id)
        return json.dumps(expected)

    monkeypatch.setattr(
        catalog._native_catalog, "resolve_firmware", resolve, raising=False
    )

    assert catalog.resolve_firmware("x70-110") == expected
    assert calls == ["x70-110"]


def test_configuration_resolution_returns_its_validated_reference(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    expected = {
        "configuration": {
            "id": "x70-110-rec",
            "body_id": "x70",
            "firmware_id": "x70-110",
        },
        "reference": {
            "soc": {"id": "exr2"},
            "body": {"id": "x70"},
            "firmware": {"id": "x70-110"},
            "parts": [],
        },
    }
    calls = []

    def resolve(configuration_id: str, overlays: list[str]) -> str:
        calls.append((configuration_id, overlays))
        return json.dumps(expected)

    monkeypatch.setattr(
        catalog._native_catalog, "resolve_configuration", resolve, raising=False
    )

    result: catalog.ResolvedConfiguration = catalog.resolve_configuration(
        "x70-110-rec", overlays=("x70-110-rec-usb",)
    )

    assert result == expected
    assert calls == [("x70-110-rec", ["x70-110-rec-usb"])]


def test_selection_requires_native_json_object(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        catalog._native_catalog,
        "resolve_reference",
        lambda body_id, firmware_id: "[]",
        raising=False,
    )

    with pytest.raises(RuntimeError, match="must return a JSON object"):
        catalog.resolve_reference("x70", "x70-110")


def test_selection_requires_native_json_text(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        catalog._native_catalog,
        "resolve_firmware",
        lambda firmware_id: {"id": firmware_id},
        raising=False,
    )

    with pytest.raises(RuntimeError, match="must return JSON text"):
        catalog.resolve_firmware("x70-110")


def test_configuration_requires_native_json_object(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        catalog._native_catalog,
        "resolve_configuration",
        lambda configuration_id, overlays: "[]",
        raising=False,
    )

    with pytest.raises(RuntimeError, match="must return a JSON object"):
        catalog.resolve_configuration("x70-110-rec")


@pytest.mark.parametrize(
    ("body_id", "firmware_id"),
    [(1, "x70-110"), ("x70", None)],
)
def test_reference_resolution_rejects_non_string_ids(
    body_id: object, firmware_id: object
) -> None:
    with pytest.raises(TypeError, match="must be strings"):
        catalog.resolve_reference(cast(str, body_id), cast(str, firmware_id))


def test_configuration_resolution_rejects_non_string_id() -> None:
    with pytest.raises(TypeError, match="configuration_id must be a string"):
        catalog.resolve_configuration(cast(str, 3))


@pytest.mark.parametrize(
    "overlays",
    ["x70-110-rec-usb", ["UPPER"], ["x70-110-rec-usb", "x70-110-rec-usb"]],
)
def test_configuration_resolution_rejects_invalid_overlays(overlays: object) -> None:
    with pytest.raises((TypeError, ValueError)):
        catalog.resolve_configuration("x70-110-rec", overlays=cast(list[str], overlays))


def test_generated_typed_dicts_import_and_resolve_forward_references() -> None:
    hints = {
        name: get_type_hints(value)
        for name, value in vars(_catalog_types).items()
        if is_typeddict(value)
    }

    assert _catalog_types.CardSlotAssembly in get_args(hints["Body"]["card_slot"])
