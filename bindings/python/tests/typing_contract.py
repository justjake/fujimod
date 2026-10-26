"""Checker regression cases for the generated Rust and JavaScript boundaries."""

from typing import TYPE_CHECKING, assert_type

if TYPE_CHECKING:
    from pathlib import Path

    from fujimod import usb
    from fujimod.emulator import (
        Board,
        BootReport,
        ExecutionPolicy,
        ScanoutFrame,
        Session,
    )
    from fujimod.usb import app80

    def generated_values() -> None:
        bank = app80.PinBank(2)
        assert_type(bank.value, int)
        assert_type(app80.Command.ReadPins(bank), app80.Command.ReadPins)
        assert_type(app80.PeerAddress(bytearray(6)).data, bytes)
        app80.Command.ReadPins(2)  # ty: ignore[invalid-argument-type]  # Enforce the checked quantity boundary.
        bank.value = 3  # ty: ignore[invalid-assignment]  # Generated quantities are immutable.

    async def bridge_results(session: usb.Session) -> None:
        assert_type(await session.read_memory(0, 4), bytes)
        assert_type(await session.device_info(usb.InfoSelector.Product), str)
        await session.device_info(usb.CameraMode.Record)  # ty: ignore[invalid-argument-type]  # Enum groups remain distinct across the bridge.
        session.missing_operation()  # ty: ignore[unresolved-attribute]  # The bridge must not silently become dynamic.

    def emulator_inputs() -> None:
        board = Board.from_inputs(
            configuration_id="x70-110-rec",
            overlays=("x70-110-rec-usb",),
            update=Path("update.dat"),
            boot_rom=Path("boot.rom"),
            nand=Path("nand.bin"),
            settings=Path("settings.bin"),
        )
        assert_type(board, Board)
        assert_type(board.boot_to_rec(), BootReport)
        assert_type(board.closed, bool)
        assert_type(board.execution_policy, ExecutionPolicy)
        board.set_execution_policy("exact")
        board.set_execution_policy("interactive")
        session = board.session
        assert_type(session, Session)
        assert_type(session.adc_samples("adc0"), tuple[int, ...])
        assert_type(session.scanout("display0"), ScanoutFrame | None)
        assert_type(session.screenshot(), bytes)
        board.read_memory(0, 4)
        board.write_memory(0, bytearray(4))
