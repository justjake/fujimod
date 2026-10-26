"""Real native store stops and checkpoint replay through the Python API."""
import hashlib
import json
import struct

import pytest
from fujimod.emulator import Asset, Definition, Image, make_board
from fujimod.emulator.waits import wait_until


def board(quantum):
    # add r0,r0,#1; str r0,[r1]; b loop
    program = struct.pack('<3I', 0xe2800001, 0xe5810000, 0xeafffffc)
    asset = Asset('program', len(program), hashlib.sha256(program).hexdigest())
    graph = dict(processors=[dict(model='cortex-r5f', released=True, mprofile=None,
        rprofile=dict(cp14_identity=0, tcm=dict(atcm=dict(bytes=8192, reset=0xffff8001),
                                             btcm=dict(bytes=4096, reset=0x70000001))),
        registers=[['Pc',0x10000],['Pstate',0xd3]])],
        ram=[dict(base=0x10000,size=0x4000)],
        dma_mappings=[dict(address=0x10000,physical=0x10000,size=0x4000,permissions=3)],
        decoders=[], mappings=[dict(core=0,address=0x10000,physical=0x10000,size=0x4000,permissions=7)],
        peripherals=[dict(core=0,address=0xfff86000,physical=0xfff86000,size=0x1000)], instructions_per_tick=[1,1])
    plan = dict(schema=1,boot_entry='loader',wires={},nodes=[dict(name='idle',
        boot_entries=['rom','loader'], windows=[dict(base=0xfff86000,size=12)], dependencies=[],
        model=dict(kind='block86000',options={}))])
    result = make_board(Definition(execution=graph, devices=plan, assets=(asset,),
        images=(Image(asset.id,0x10000,core=0),), quantum=quantum),
        {asset.id:program})
    result.cpus[0].reg_write({'Arm32':{'R':1}},0x12000)
    return result


@pytest.mark.parametrize('quantum',[1,7,64])
def test_native_store_stop_finishes_the_store_before_the_next_instruction(quantum):
    with board(quantum) as machine:
        assert machine.run(20,store_watches=[(0x12000,0x12004)])
        assert machine.instructions == 2
        assert machine.virtual_ticks == 2
        assert machine.read_physical(0x12000,4) == struct.pack('<I',1)
        assert machine.cpus[0].reg_read('Pc') == 0x10008
        wait_until(machine,lambda:int.from_bytes(machine.read_physical(0x12000,4),'little') >= 4,
                   'fourth stored counter',instruction_limit=30)
        assert machine.instructions == 11


@pytest.mark.parametrize('quantum',[1,7,64])
def test_native_watches_preserve_counts_ticks_and_complete_fingerprint(quantum):
    with board(quantum) as machine:
        saved = machine.checkpoint()
        results = []
        for watched in (False,True):
            machine.restore_checkpoint(saved)
            while machine.instructions < 90:
                machine.run(90-machine.instructions,store_watches=[(0x12000,0x12004)] if watched else None)
            results.append((machine.instructions,machine.virtual_ticks,machine.fingerprint()))
        assert results[0] == results[1]
        machine.native.read_set_begin()
        machine.read_physical(0x12000,4)
        assert json.loads(machine.native.read_set_end()) == dict(stores=[[0x12000,0x12004]],devices=[])



def test_native_device_publication_stops_and_read_sets_resolve_graph_names():
    with board(7) as machine:
        machine.native.read_set_begin()
        machine.native.device_states('exr2-block-86000')
        assert json.loads(machine.native.read_set_end()) == dict(stores=[],devices=['idle'])
        machine.write_memory(0x10000,struct.pack('<3I',0xe5810000,0xe3a00063,0xeafffffe))
        machine.cpus[0].reg_write({'Arm32':{'R':0}},0x300)
        machine.cpus[0].reg_write({'Arm32':{'R':1}},0xfff86000)
        assert machine.run(20,device_watches=['idle'])
        assert machine.instructions == 1
        assert machine.cpus[0].reg_read('Pc') == 0x10004
        assert machine.cpus[0].reg_read({'Arm32':{'R':0}}) == 0x300
