"""Every whole-board run frontend advances queued physical contacts."""
import hashlib
import json
import struct

import pytest
from fujimod.emulator import Asset, Definition, Image, make_board

from test_emulator import _execution_graph


def contact_board(quantum):
    # Acknowledge press, then release, through guest stores to the same byte.
    program = struct.pack("<5I", 0xe3a00001, 0xe5c10000, 0xe2800001,
                          0xe5c10000, 0xeafffffe)
    asset = Asset("program", len(program), hashlib.sha256(program).hexdigest())
    graph = _execution_graph()
    graph["processors"][0]["registers"].append(["Pstate", 0xd3])
    graph["dma_mappings"] = [dict(address=0x10000, physical=0x10000, size=0x4000, permissions=3)]
    graph["peripherals"] = [dict(core=0, address=0xfff8a000,
                                  physical=0xfff8a000, size=0x60)]
    plan = dict(schema=1, boot_entry="loader", wires={}, nodes=[dict(
        name="adc", boot_entries=["rom", "loader"], dependencies=[],
        windows=[dict(base=0xfff8a000, size=0x60)],
        model=dict(kind="adc", options=dict(conversion_ticks=1, samples=[1023] * 12)))])
    machine = make_board(Definition(execution=graph, devices=plan, assets=(asset,),
        images=(Image(asset.id, 0x10000, core=0),), quantum=quantum), {asset.id: program})
    machine.cpus[0].reg_write({"Arm32": {"R": 1}}, 0x12000)
    machine.write_memory(0x12001, b"\x01")
    controls = dict(
        controls=[dict(id="menu", label="MENU", kind="button",
                       positions=["released", "pressed"], initial="released", unavailable=None)],
        circuits=[dict(controls=["menu"], cases=[dict(positions=[position], signals=[
            dict(kind="analog", node="adc", channel=8, value=value)])
            for position, value in (("released", 1023), ("pressed", 21))])])
    machine.native.configure_controls(json.dumps(controls))
    driver = dict(
        poll_ticks=1, max_phase_ticks=100, interfaces=[],
        catalog=[dict(id="menu", label="MENU", kind="button", supported=True,
                      unavailable=None, positions=None, initial=None, relative=None,
                      width=None, height=None, stages=None)],
        actuators=[dict(id="menu", momentary=True, states=[dict(
            position=position, ready=[dict(address=0x12001, bytes=[1], pointer=None)],
            expected=[dict(address=0x12000,
                                                       bytes=[ack], pointer=None)])
            for position, ack in (("released", 2), ("pressed", 1))])])
    machine.native.configure_contact_driver(json.dumps(driver))
    machine.native.contact_input("menu", None, False)
    machine.native.contact_input("menu", None, True)
    return machine


@pytest.mark.parametrize("quantum", [1, 7, 64])
@pytest.mark.parametrize("frontend", ["budget", "window", "instructions", "tick", "soft_pause"])
def test_one_native_run_completes_queued_press_and_release(quantum, frontend):
    with contact_board(quantum) as machine:
        if frontend == "budget":
            machine.run(40)
        elif frontend == "window":
            machine.native.run_window(40, quantum)
        elif frontend == "instructions":
            machine.native.run_for_instructions(40, quantum)
        elif frontend == "tick":
            machine.run_until(40)
        else:
            machine.run(80, pause_after=40)
        queue = json.loads(machine.native.control_state())["inputQueue"]
        assert queue["active"] is None
        assert queue["pending"] == 0
        assert queue["completed"] == 2
        assert json.loads(machine.native.control_state())["positions"]["menu"] == "released"
        expected = ((40 + quantum - 1) // quantum) * quantum if frontend == "soft_pause" else 40
        assert machine.instructions == expected
        assert machine.virtual_ticks == expected


def test_queued_contact_polling_preserves_passive_store_stops():
    with contact_board(7) as machine:
        assert machine.run(40, store_watches=[(0x12000, 0x12001)])
        assert machine.instructions == 2
        assert machine.read_physical(0x12000, 1) == b"\x01"
        assert machine.run(38, store_watches=[(0x12000, 0x12001)])
        assert machine.instructions == 4
        assert machine.read_physical(0x12000, 1) == b"\x02"
        assert json.loads(machine.native.control_state())["inputQueue"]["completed"] == 2
