"""The camera builder's open through the Python bindings."""

import hashlib
import inspect
import json
import os
from pathlib import Path

import pytest
from fujimod.emulator import Board

PNG = b"\x89PNG\r\n\x1a\n"


def root() -> Path:
    root = Path(os.environ.get("FUJIMOD_FIRMWARE_LIBRARY", "."))
    record = root / "firmware/x70/1.10/firmware.json"
    if not record.is_file():
        pytest.skip("the library holds no x70-110 source record")
    return root


def library() -> Path:
    directory = root()
    record = directory / "firmware/x70/1.10/firmware.json"
    if "boot_rom" not in json.loads(record.read_text()):
        pytest.skip("the x70-110 record has no boot ROM")
    return directory


def x70_files(root: Path) -> tuple[Path, Path]:
    """The X70 update and boot ROM the source record names."""
    record = json.loads((root / "firmware/x70/1.10/firmware.json").read_text())
    update = root / str(record["source"]["path"])
    rom = record.get("boot_rom")
    boot_rom = root / str(rom["source"]["path"] if rom else record["paths"]["boot_rom"])
    if not update.is_file() or not boot_rom.is_file():
        pytest.skip("the X70 source record's files are absent")
    return update, boot_rom


def release_card() -> Path:
    state = Path(os.environ.get("FUJIMOD_STATE_DIR", "/tmp/fujimod"))
    cards = sorted(state.glob("sd-cards/*/x70-110"))
    if not cards:
        pytest.skip("the firmware library holds no release X70 SD card")
    return cards[-1]


def test_open_camera_refuses_unknown_options() -> None:
    with pytest.raises(ValueError, match="boot"):
        Board.open_camera(body="x70", boot="sd")  # ty: ignore[invalid-argument-type]  # deliberately invalid
    with pytest.raises(ValueError, match="execution_policy"):
        Board.open_camera(body="x70", execution_policy="fast")  # ty: ignore[invalid-argument-type]  # deliberately invalid
    with pytest.raises(TypeError, match="boot_cache"):
        Board.open_camera(body="x70", boot_cache=1)  # ty: ignore[invalid-argument-type]  # deliberately invalid
    with pytest.raises(TypeError, match="run_to_rec"):
        Board.open_camera(body="x70", run_to_rec=1)  # ty: ignore[invalid-argument-type]  # deliberately invalid
    with pytest.raises(ValueError, match="boolean"):
        Board.open_camera(body="x70", sd_card={"present": "yes"})
    with pytest.raises(RuntimeError, match="transfer ticks"):
        Board.open_camera(body="x70", sd_card={"transfer_ticks": 0})
    with pytest.raises(ValueError, match="autostart"):
        Board.open_camera(body="x70", boot="autostart", run_to_rec=False)
    with pytest.raises(RuntimeError, match="X-H2 firmware, not X-H2S"):
        Board.open_camera(body="xh2s", firmware="xh2-530")
    with pytest.raises(RuntimeError, match="no sd_card_2 slot"):
        Board.open_camera(body="x70", sd_card_2={})
    with pytest.raises(RuntimeError, match="CFexpress is not modeled yet"):
        Board.open_camera(body="xh2s", cfexpress_card={})


def test_the_x70_stock_boot_opens_to_rec() -> None:
    library()
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
    ) as board:
        report = board.status()
        assert report["status"] == "rec"
        assert report["pending"] is False
        assert board.quantum > 0
        assert board.session.screenshot().startswith(PNG)
        with pytest.raises(RuntimeError, match="from_inputs"):
            board.boot_to_rec()


def test_the_x70_release_card_opens_to_rec() -> None:
    card = release_card()
    with Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card},
        execution_policy="interactive",
    ) as board:
        assert board.status()["status"] == "rec"


def held_positions(board: Board) -> dict[str, object]:
    positions = board.session.control_state()["positions"]
    assert isinstance(positions, dict)
    return {
        name: positions[name]
        for name in ("command_control.press", "movie_record_button")
    }


def test_the_x70_card_boot_can_stop_before_the_first_instruction() -> None:
    card = release_card()
    with Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card},
        execution_policy="interactive",
        run_to_rec=False,
    ) as board:
        report = board.status()
        assert report["status"] != "rec"
        assert report["instructions"] == 0
        assert report["virtual_ticks"] == 0
        assert held_positions(board) == {
            "command_control.press": "pressed",
            "movie_record_button": "pressed",
        }
        for _ in range(200):
            if board.status()["status"] == "rec":
                break
            board.run(limit=20_000_000)
        else:
            raise AssertionError("the camera did not reach REC")
        assert held_positions(board) == {
            "command_control.press": "pressed",
            "movie_record_button": "pressed",
        }
        board.release_holds()
        assert held_positions(board) == {
            "command_control.press": "released",
            "movie_record_button": "released",
        }
        board.release_holds()
        assert board.status()["status"] == "rec"


def test_an_absent_sd_card_boots_and_reports_the_slot_empty() -> None:
    library()
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        sd_card={"present": False},
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["present"] is False
        assert state["protected"] is False


def test_a_write_protected_sd_card_boots_and_reports_protection() -> None:
    library()
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        sd_card={"write_protected": True},
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["present"] is True
        assert state["protected"] is True


def test_sd_card_transfer_ticks_slow_the_card_boot() -> None:
    card = release_card()
    default = Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card},
        execution_policy="interactive",
        boot_cache=False,
    )
    slow = Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card, "transfer_ticks": 20_000},
        execution_policy="interactive",
        boot_cache=False,
    )
    try:
        assert default.status()["status"] == "rec"
        assert slow.status()["status"] == "rec"
        assert slow.status()["virtual_ticks"] > default.status()["virtual_ticks"]
    finally:
        default.close()
        slow.close()


def test_a_declared_card_layout_formats_the_stock_card() -> None:
    library()
    layout = {
        "sectors": 8_388_608,
        "label": "LAYOUT",
        "partition_start": 16_384,
        "sectors_per_cluster": 32,
    }
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        sd_card={"layout": layout},
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["capacity_bytes"] == layout["sectors"] * 512
    card = release_card()
    with Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card, "layout": layout},
        execution_policy="interactive",
        boot_cache=False,
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["capacity_bytes"] == layout["sectors"] * 512
    with pytest.raises(ValueError, match="missing field"):
        Board.open_camera(body="x70", sd_card={"layout": {"sectors": 8_388_608}})
    with pytest.raises(ValueError, match="invalid type"):
        Board.open_camera(body="x70", sd_card={"layout": {**layout, "sectors": "big"}})
    with pytest.raises(RuntimeError, match="card present"):
        Board.open_camera(body="x70", sd_card={"present": False, "layout": layout})
    with pytest.raises(RuntimeError, match="cluster size"):
        Board.open_camera(
            body="x70",
            sd_card={"layout": {**layout, "sectors_per_cluster": 3}},
        )


def test_the_x70_powers_on_with_its_flash_and_card_kept() -> None:
    card = release_card()
    with Board.open_camera(
        body="x70",
        boot="sd-card",
        sd_card={"directory": card},
        execution_policy="interactive",
    ) as board:
        assert board.status()["status"] == "rec"
        board.power_on()
        assert board.status()["status"] == "rec"
        board.power_on(hold_controls=True)
        assert board.status()["status"] == "rec"
        board.power_on(hold_controls=True, fresh_card=True)
        assert board.status()["status"] == "rec"
        with pytest.raises(TypeError, match="fresh_card"):
            board.power_on(fresh_card=1)  # ty: ignore[invalid-argument-type]  # deliberately invalid


def test_a_stock_boot_cannot_power_on_holding_card_controls() -> None:
    library()
    with (
        Board.open_camera(
            body="x70",
            boot="stock",
            execution_policy="interactive",
            boot_cache=False,
        ) as board,
        pytest.raises(RuntimeError, match="SD card source"),
    ):
        board.power_on(hold_controls=True)


def test_a_verified_image_starts_at_the_loader(tmp_path: Path) -> None:
    library()
    image = b"fujimod loader image"
    path = tmp_path / "image.bin"
    path.write_bytes(image)
    digest = hashlib.sha256(image).hexdigest()
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        run_to_rec=False,
        loader_image={"path": path, "sha256": digest},
    ) as board:
        assert board.status()["instructions"] == 0
        assert board.read_physical(0, len(image)) == image
        assert board.cpus[0].reg_read("Pc") == 0
    with pytest.raises(RuntimeError, match="sha256"):
        Board.open_camera(
            body="x70",
            run_to_rec=False,
            loader_image={"path": path, "sha256": "0" * 64},
        )
    with pytest.raises(TypeError, match="loader_image"):
        Board.open_camera(body="x70", loader_image={"path": path})
    with pytest.raises(ValueError, match="alternative starts"):
        Board.open_camera(
            body="x70", hold="sd", loader_image={"path": path, "sha256": digest}
        )


def test_a_camera_opens_from_named_files_and_card_bytes() -> None:
    directory = root()
    update, boot_rom = x70_files(directory)
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        firmware_files={"update": update, "boot_rom": boot_rom},
        sd_card={"files": {"NOTE.TXT": {"bytes": b"written from memory"}}},
    ) as board:
        assert board.status()["status"] == "rec"
    with pytest.raises(ValueError, match="alternatives"):
        Board.open_camera(
            body="x70",
            sd_card={"directory": directory, "files": {"NOTE.TXT": {"bytes": b"x"}}},
        )


def test_a_camera_opened_from_named_files_powers_on_from_them(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    directory = root()
    update, boot_rom = x70_files(directory)
    with Board.open_camera(
        body="x70",
        firmware="1.10",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        firmware_files={"update": update, "boot_rom": boot_rom},
    ) as board:
        monkeypatch.delenv("FUJIMOD_FIRMWARE_LIBRARY")
        board.power_on()
        assert board.status()["status"] == "rec"


def test_the_x70_request_document_matches_the_shared_schema() -> None:
    schema = json.loads(
        (
            Path(__file__).resolve().parents[3]
            / "catalog/schemas/emulator-camera.generated.schema.json"
        ).read_text()
    )
    assert schema["required"] == ["body"]
    names = set(inspect.signature(Board.open_camera).parameters)
    assert names == set(schema["properties"])
