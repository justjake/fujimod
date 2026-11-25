"""The camera builder's open through the Python bindings."""

import os
from pathlib import Path

import pytest
from fujimod.emulator import Board

PNG = b"\x89PNG\r\n\x1a\n"


def library() -> Path:
    root = os.environ.get("FUJIMOD_FIRMWARE_LIBRARY")
    if not root:
        pytest.skip("set FUJIMOD_FIRMWARE_LIBRARY to a firmware library")
    return Path(root)


def release_card(root: Path) -> Path:
    cards = sorted(root.glob("sd-cards/*/x70-110"))
    if not cards:
        pytest.skip("the firmware library holds no release X70 SD card")
    return cards[-1]


def test_open_camera_refuses_unknown_options() -> None:
    with pytest.raises(ValueError, match="boot"):
        Board.open_camera(firmware="x70-110", boot="sd")
    with pytest.raises(ValueError, match="execution_policy"):
        Board.open_camera(firmware="x70-110", execution_policy="fast")
    with pytest.raises(TypeError, match="boot_cache"):
        Board.open_camera(firmware="x70-110", boot_cache=1)
    with pytest.raises(TypeError, match="run_to_rec"):
        Board.open_camera(firmware="x70-110", run_to_rec=1)
    with pytest.raises(TypeError, match="sd_card_present"):
        Board.open_camera(firmware="x70-110", sd_card_present="yes")
    with pytest.raises(TypeError, match="sd_card_write_protected"):
        Board.open_camera(firmware="x70-110", sd_card_write_protected=None)
    with pytest.raises(ValueError, match="sd_card_transfer_ticks"):
        Board.open_camera(firmware="x70-110", sd_card_transfer_ticks=0)
    with pytest.raises(ValueError, match="automatic"):
        Board.open_camera(firmware="x70-110", boot="automatic-boot", run_to_rec=False)
    with pytest.raises(ValueError, match="present"):
        Board.open_camera(firmware="x70-110", boot="sd-card", sd_card_present=False)


def test_the_x70_stock_boot_opens_to_rec() -> None:
    library()
    with Board.open_camera(
        firmware="x70-110",
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
    card = release_card(library())
    with Board.open_camera(
        firmware="x70-110",
        boot="sd-card",
        sd_card=card,
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
    card = release_card(library())
    with Board.open_camera(
        firmware="x70-110",
        boot="sd-card",
        sd_card=card,
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
        firmware="x70-110",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        sd_card_present=False,
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["present"] is False
        assert state["protected"] is False


def test_a_write_protected_sd_card_boots_and_reports_protection() -> None:
    library()
    with Board.open_camera(
        firmware="x70-110",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
        sd_card_write_protected=True,
    ) as board:
        assert board.status()["status"] == "rec"
        state = board.session.card_state("sd")
        assert state["present"] is True
        assert state["protected"] is True


def test_sd_card_transfer_ticks_slow_the_card_boot() -> None:
    card = release_card(library())
    default = Board.open_camera(
        firmware="x70-110",
        boot="sd-card",
        sd_card=card,
        execution_policy="interactive",
        boot_cache=False,
    )
    slow = Board.open_camera(
        firmware="x70-110",
        boot="sd-card",
        sd_card=card,
        execution_policy="interactive",
        boot_cache=False,
        sd_card_transfer_ticks=20_000,
    )
    try:
        assert default.status()["status"] == "rec"
        assert slow.status()["status"] == "rec"
        assert slow.status()["virtual_ticks"] > default.status()["virtual_ticks"]
    finally:
        default.close()
        slow.close()


def test_the_x70_powers_on_with_its_flash_and_card_kept() -> None:
    card = release_card(library())
    with Board.open_camera(
        firmware="x70-110",
        boot="sd-card",
        sd_card=card,
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
            board.power_on(fresh_card=1)


def test_a_stock_boot_cannot_power_on_holding_card_controls() -> None:
    library()
    with Board.open_camera(
        firmware="x70-110",
        boot="stock",
        execution_policy="interactive",
        boot_cache=False,
    ) as board:
        with pytest.raises(RuntimeError, match="SD card source"):
            board.power_on(hold_controls=True)
