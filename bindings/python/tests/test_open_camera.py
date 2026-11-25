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
