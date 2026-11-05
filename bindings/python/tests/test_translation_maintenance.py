"""Stopped XP4 maintenance diagnostics preserve outcomes without advancing CPUs."""

import hashlib
import json
import struct
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TypedDict, cast

import pytest
from fujimod._native import emulator as native
from fujimod._native.emulator import NativeError
from fujimod.emulator import Asset, Board, Image, Xp4Definition, make_board


class MaintenanceEvent(TypedDict):
    owner: int
    sequence: int
    vttbr: int
    hcr: int
    cpsr: int
    scr: int
    nsacr: int
    actlr: int
    exception_level: int
    secure: int
    selector: int
    operand: int


class NativeMaintenance(TypedDict):
    owner: int
    next_sequence: int
    pending: int
    poison: int
    front: MaintenanceEvent
    failed: MaintenanceEvent


class MaintenanceDelivery(TypedDict):
    source_core: int
    source_identity: int
    event: MaintenanceEvent
    peer_actlr: list[int]
    recipients: int
    applied: int
    excluded: int
    acknowledged: bool
    error: str | None
    raw_error: str | None


class MaintenanceSnapshot(TypedDict):
    identities: list[int]
    native: list[NativeMaintenance]
    completed: int
    delivered_peers: int
    dropped: int
    records: list[MaintenanceDelivery]
    pending: MaintenanceDelivery | None
    failure: str | None
    last_raw_error: str | None


@contextmanager
def machine(
    cores: int, words: tuple[int, ...] = (0xEE080F13, 0xE1A00000)
) -> Iterator[tuple[Board, native.Xp4]]:
    program = struct.pack("<" + "I" * len(words), *words)
    asset = Asset("program", len(program), hashlib.sha256(program).hexdigest())
    board = make_board(
        Xp4Definition(
            options={
                "ram_size": 0x10000,
                "cores": cores,
                "discovery": False,
                "cbar": 0x20000000,
                "initial_registers": [
                    [["Pc", 0x1000], ["Pstate", 0xD3]] for _ in range(cores)
                ],
            },
            assets=(asset,),
            images=(Image(asset.id, 0x1000),),
            quantum=1,
        ),
        {asset.id: program},
    )
    try:
        for cpu in board.cpus:
            cpu.cpr_write(15, 0, 1, 1, 0, 1, False, 0)  # SCR: Secure.
            cpu.cpr_write(15, 0, 1, 0, 1, 1, False, 0x6040)  # ACTLR.SMP.
        yield board, board.native.xp4
    finally:
        board.close()


def observation(
    board: Board, xp4: native.Xp4
) -> tuple[
    dict[str, object],
    bytes,
    tuple[tuple[int, int], ...],
    list[dict[str, int | bool]],
    dict[str, str],
]:
    return (
        xp4.state(False),
        bytes(xp4.read(0, 0x10000)),
        tuple((cpu.reg_read("Pc"), cpu.reg_read("Pstate")) for cpu in board.cpus),
        xp4.interrupt_state(),
        xp4.device_state_json(),
    )


def snapshot_without_effects(board: Board, xp4: native.Xp4) -> MaintenanceSnapshot:
    before = observation(board, xp4)
    first = cast(MaintenanceSnapshot, xp4.translation_maintenance_state())
    assert first == xp4.translation_maintenance_state()
    assert observation(board, xp4) == before
    assert json.loads(json.dumps(first)) == first
    return first


@pytest.mark.parametrize("cores", [2, 4])
def test_empty_snapshot_is_detached_and_does_not_advance_execution(cores: int) -> None:
    with machine(cores) as (board, xp4):
        saved = snapshot_without_effects(board, xp4)
        assert len(set(saved["identities"])) == cores
        assert all(type(value) is int and value > 0 for value in saved["identities"])
        assert saved["completed"] == saved["delivered_peers"] == saved["dropped"] == 0
        assert saved["records"] == []
        assert saved["pending"] is saved["failure"] is saved["last_raw_error"] is None
        assert len(saved["native"]) == cores
        for native in saved["native"]:
            assert native["owner"] > 0
            assert native["next_sequence"] == 1
            assert native["pending"] == native["poison"] == 0
            assert len(native["front"]) == len(native["failed"]) == 12
            assert set(native["front"].values()) == set(native["failed"].values()) == {0}
        saved["identities"][0] = -1
        saved["native"][0]["front"]["sequence"] = -1
        fresh = cast(MaintenanceSnapshot, xp4.translation_maintenance_state())
        assert fresh["identities"][0] > 0
        assert fresh["native"][0]["front"]["sequence"] == 0


@pytest.mark.parametrize("cores", [2, 4])
def test_completed_delivery_preserves_source_metadata_and_peer_masks(cores: int) -> None:
    with machine(cores) as (board, xp4):
        before = cast(MaintenanceSnapshot, xp4.translation_maintenance_state())
        xp4.run(0, 1)
        saved = snapshot_without_effects(board, xp4)
        assert saved["completed"] == 1
        assert saved["delivered_peers"] == cores - 1
        assert saved["dropped"] == 0
        assert saved["pending"] is saved["failure"] is saved["last_raw_error"] is None
        assert len(saved["records"]) == 1
        record = saved["records"][0]
        assert record["source_core"] == 0
        assert record["source_identity"] == before["identities"][0]
        assert record["peer_actlr"] == [0x6040] * cores
        assert record["recipients"] == record["applied"] == (1 << cores) - 2
        assert record["excluded"] == 0
        assert record["acknowledged"] is True
        assert record["error"] is record["raw_error"] is None
        event = record["event"]
        assert event["owner"] == before["native"][0]["owner"]
        assert event["sequence"] == 1
        assert event["selector"] == 0x0F083000
        assert event["operand"] == 0
        assert event["actlr"] == 0x6040
        assert event["cpsr"] == 0xD3
        assert event["scr"] == event["nsacr"] == event["hcr"] == event["vttbr"] == 0
        assert (event["secure"], event["exception_level"]) == (1, 3)
        assert saved["native"][0]["next_sequence"] == 2
        assert all(item["pending"] == item["poison"] == 0 for item in saved["native"])
        assert xp4.state(False)["per_core"] == [1] + [0] * (cores - 1)


def test_failed_delivery_keeps_pending_event_and_exact_error_inspectable() -> None:
    with machine(4) as (board, xp4):
        board.cpus[2].cpr_write(15, 0, 1, 0, 1, 1, False, 0xE040)
        with pytest.raises(NativeError, match="unsupported ACTLR.DDVM") as refused:
            xp4.run(0, 1)
        saved = snapshot_without_effects(board, xp4)
        assert saved["failure"] == str(refused.value)
        assert saved["completed"] == saved["delivered_peers"] == 0
        assert saved["records"] == []
        assert saved["native"][0]["pending"] == 1
        pending = saved["pending"]
        assert pending is not None
        assert pending["event"] == saved["native"][0]["front"]
        assert pending["error"] == saved["failure"]
        assert pending["applied"] == 0
        assert pending["acknowledged"] is False
        assert pending["peer_actlr"] == [0x6040, 0x6040, 0xE040, 0x6040]
        with pytest.raises(NativeError) as repeated:
            xp4.run(1, 1)
        assert str(repeated.value) == str(refused.value)
        assert snapshot_without_effects(board, xp4) == saved


def test_snapshot_refuses_during_instruction_callback_and_recovers_when_stopped() -> None:
    with machine(2, (0xE1A00000,)) as (board, xp4):
        seen: list[tuple[int, int, str]] = []

        def callback(core: int, address: int) -> None:
            with pytest.raises(NativeError) as refused:
                xp4.translation_maintenance_state()
            seen.append((core, address, str(refused.value)))

        xp4.set_pc_handler(callback)
        xp4.instruction_points(0, [0x1000])
        xp4.run(0, 1)
        assert len(seen) == 1
        assert seen[0][:2] == (0, 0x1000)
        assert seen[0][2]
        assert snapshot_without_effects(board, xp4)["failure"] is None
        assert xp4.state(False)["per_core"] == [1, 0]


def test_native_failure_keeps_accepted_event_and_raw_error_diagnostics() -> None:
    with machine(2, (0xEE080F13, 0xE5901000)) as (board, xp4):
        board.cpus[0].reg_write({"Arm32": {"R": 0}}, 0x10000000)
        with pytest.raises(NativeError):
            xp4.run(0, 2)
        saved = snapshot_without_effects(board, xp4)
        assert saved["completed"] == saved["delivered_peers"] == 1
        assert saved["last_raw_error"]
        assert saved["records"][0]["raw_error"] == saved["last_raw_error"]
        assert saved["records"][0]["acknowledged"] is True
        assert saved["pending"] is None
