"""Explicit input bounds and modification receipts for isolated ARM calls."""

from dataclasses import replace
import struct

import pytest

from fujimod.emulator import Definition, make_board
from fujimod.emulator.driver_harness import DriverHarness
from fujimod.emulator.harness_inputs import CallerLayout, CodeImage, HarnessInputs
from fujimod.emulator.harness_memory import MemoryLayout
from fujimod.emulator.routine_board import RoutineBoard

CODE = 0x10000
PAYLOAD = 0x14000
ALIAS = 0x50000000


def words(*values):
    return struct.pack('<' + 'I' * len(values), *values)


class Labels:
    def entries(self, name):
        return (CODE,) if name == 'store_argument' else ()

    def ranges(self, entry):
        return ((CODE, CODE + 8),) if entry == CODE else None

    def containing(self, address):
        return (CODE, 'store_argument') if CODE <= address < CODE + 8 else None


@pytest.fixture
def inputs():
    graph = {
        'processors': [{
            'model': 'cortex-a7', 'released': True, 'registers': [],
            'mprofile': None, 'rprofile': None,
        }],
        'ram': [{'base': CODE, 'size': 0x10000}],
        'mappings': [
            {'core': 0, 'address': address, 'physical': CODE,
             'size': 0x10000, 'permissions': 7}
            for address in (CODE, ALIAS)
        ],
        'dma_mappings': [], 'peripherals': [], 'decoders': [],
        'instructions_per_tick': [1, 1],
    }
    definition = Definition(execution=graph, devices={'kind': 'none'}, assets=())
    with make_board(definition, {}) as board:
        caller = RoutineBoard(board, cpu_identity=(None,), device_graph={},
                              input_manifest={'scene': {}, 'assets': {}})
        code = words(0xE5810000, 0xE12FFF1E) + bytes(24)  # str r0,[r1]; bx lr
        yield HarnessInputs(
            board=caller,
            memory=MemoryLayout((CODE, 0x20000), ((ALIAS, CODE, 0x10000),), ()),
            images=(CodeImage('authored', CODE, code),),
            reserved_images=((CODE, CODE + len(code)),),
            caller=CallerLayout(0x1FF00, 0x1B000, (0x1C000, 0x1F000)),
            symbols=Labels(), observed_memory=((CODE, 0x20000),),
        )


@pytest.mark.parametrize('changes', [
    {'name': ''}, {'address': -1}, {'address': (1 << 32) - 1},
    {'address': True}, {'data': b''}, {'data': bytearray(4)}, {'readonly': 1},
])
def test_code_image_retains_name_byte_address_and_readonly_checks(changes):
    with pytest.raises(ValueError):
        replace(CodeImage('code', CODE, words(0xE12FFF1E)), **changes)


def test_readonly_images_require_resident_bytes_and_refuse_modifications(inputs):
    image = replace(inputs.images[0], readonly=True)
    readonly = replace(inputs, images=(image,))
    with pytest.raises(ValueError, match='Read-only image differs'):
        DriverHarness(readonly, ram_fixture=())
    inputs.board.write_memory(CODE, image.data)
    with pytest.raises(ValueError, match='read-only'):
        DriverHarness(readonly, ram_fixture=(), patches=((CODE, words(0)),))
    harness = DriverHarness(readonly, ram_fixture=())
    with pytest.raises(ValueError, match='read-only'):
        harness.data_override(CODE + 16, b'data', 'test input')
    assert harness.read(CODE, len(image.data)) == image.data


def test_input_overlap_and_reservations_still_refuse(inputs):
    with pytest.raises(ValueError, match='images overlap'):
        replace(inputs, images=(*inputs.images, CodeImage('other', CODE + 4, b'data')))
    with pytest.raises(ValueError, match='reservations overlap'):
        replace(inputs, caller=replace(inputs.caller, return_address=0x1B000))


def test_patches_and_override_restore_keep_bytes_and_ordered_receipts(inputs):
    patch = words(0xE3A0002A)  # mov r0,#42
    harness = DriverHarness(inputs, ram_fixture=(), patches=((CODE, patch),))
    assert harness.patches == [{'address': CODE, 'length': 4, 'section': 'authored'}]
    assert harness.read(CODE, 8) == patch + inputs.images[0].data[4:8]
    harness.data_override(CODE + 16, b'data', 'test global')
    record = {'address': CODE + 16, 'length': 4, 'section': 'authored',
              'reason': 'test global'}
    assert harness.data_overrides == [record]
    assert harness.read(CODE + 16, 4) == b'data'
    for address in (CODE, CODE + 17):
        with pytest.raises(ValueError, match='overlaps'):
            harness.data_override(address, b'bad!', 'overlap')
    result = harness.call(CODE, allowed_code=((CODE, CODE + 8),))
    assert result['returned'] and result['return_value'] == 42
    assert result['patches'] == harness.patches
    assert result['data_overrides'] == [record]
    assert harness.data_override_restore(CODE + 16) == CODE + 16
    assert harness.read(CODE + 16, 4) == bytes(4)
    assert harness.data_overrides == []
    assert harness.data_override_events == [
        dict(record, event='override', call_index=0),
        dict(record, event='restore', call_index=1),
    ]
    with pytest.raises(ValueError, match='No active'):
        harness.data_override_restore(CODE + 16)


def test_patch_bounds_and_overlap_are_checked_before_writes(inputs):
    for patches in (
        ((CODE + 30, bytes(4)),),
        ((CODE, bytes(4)), (CODE + 2, bytes(4))),
    ):
        with pytest.raises(ValueError, match='wholly inside|overlap'):
            DriverHarness(inputs, ram_fixture=(), patches=patches)
        assert inputs.board.read_memory(CODE, 32) == inputs.images[0].data


def test_payload_replacement_keeps_tail_aliases_and_breakpoints_without_reads(
    inputs, monkeypatch
):
    payload = words(0xE3A00001, 0xE12FFF1E)  # mov r0,#1; bx lr
    harness = DriverHarness(inputs, ram_fixture=(), payloads=((PAYLOAD, payload),))
    assert harness.payloads == [{'address': PAYLOAD, 'length': len(payload)}]
    assert harness.call(PAYLOAD)['return_value'] == 1
    seen = []
    harness.observe(PAYLOAD + 4, lambda context: seen.append(context.reg('r0')))

    def unexpected_read(*args, **kwargs):
        pytest.fail('Replacing a payload must not reread it for a receipt')

    with monkeypatch.context() as scoped:
        scoped.setattr(inputs.board, 'read_memory', unexpected_read)
        record = harness.replace_payload(
            ALIAS + PAYLOAD - CODE, words(0xE3A00002), reason='second input'
        )
    assert record == {'address': PAYLOAD, 'length': 4, 'payload': PAYLOAD,
                      'reason': 'second input', 'call_index': 1}
    assert harness.read(PAYLOAD, 8) == words(0xE3A00002, 0xE12FFF1E)
    result = harness.call(PAYLOAD)
    assert result['returned'] and result['return_value'] == 2
    assert seen == [2]
    assert result['payloads'] == harness.payloads
    assert result['payload_replacements'] == [record]
    for address, data, reason in (
        (PAYLOAD + 5, bytes(4), 'past end'), (PAYLOAD - 1, b'x', 'before'),
        (PAYLOAD, b'', 'empty'), (PAYLOAD, b'x', ''),
    ):
        with pytest.raises(ValueError):
            harness.replace_payload(address, data, reason=reason)
    assert harness.read(PAYLOAD, 8) == words(0xE3A00002, 0xE12FFF1E)


@pytest.mark.parametrize('payloads', [
    ((CODE, b'data'),), ((0x1FF00, b'data'),), ((0x1C000, b'data'),),
    ((0x1B000, b'data'),), ((0x20000, b'data'),),
    ((PAYLOAD, b'data'), (PAYLOAD + 2, b'data')),
])
def test_payload_declarations_keep_reserved_and_overlap_checks(inputs, payloads):
    with pytest.raises(ValueError):
        DriverHarness(inputs, ram_fixture=(), payloads=payloads)
