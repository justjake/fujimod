#!/usr/bin/env python3
"""Probe stock DCF paths, numbering and FAT dates for X-Pro2 5.11."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM
from unicorn.arm_const import UC_ARM_REG_LR, UC_ARM_REG_PC

import arm_probe as bench

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from prepare import stock_image

parser = argparse.ArgumentParser(description=__doc__)
inputs = parser.add_mutually_exclusive_group(required=True)
inputs.add_argument('--dat', type=Path, help='Official Fujifilm X-Pro2 5.11 update')
inputs.add_argument('--resident', type=Path, help='Exact stock resident image')
args = parser.parse_args()
plan = json.loads((HERE.parent / 'data/resident-plan.json').read_text())
if args.dat:
    stock = stock_image(args.dat.read_bytes(), plan)
else:
    stock = args.resident.read_bytes()
if hashlib.sha256(stock).hexdigest() != plan['stock_sha256']:
    parser.error('X-Pro2 5.11 resident image hash mismatch')

machine = bench.machine(stock)
buffer = bench.STACK + 0x100
observations = []

for delta in (0, 0x80000):
    for suffix, expected in (('_FUJI', 0), ('_0101', 0), ('_0229', 0),
                             ('_1022', 0), ('_1231', 0), ('_10/2', 8), ('_10 2', 8)):
        data = suffix.encode('ascii')
        machine.mem_write(buffer, data)
        result = bench.call(machine, 0x957160 + delta, [buffer, len(data)])[0]
        assert result == expected and machine.mem_read(buffer, len(data)) == data
        observations.append({'validator': hex(0x957160 + delta), 'suffix': suffix, 'result': result})

    for letter in ('A', 'B'):
        for suffix in ('_FUJI', '_1022'):
            path = f'{letter}:\\DCIM\\101{suffix}\\DSCF0001.JPG'
            data = path.encode('ascii') + b'\0'
            machine.mem_write(buffer, data)
            result = bench.call(machine, 0x95b3e4 + delta, [buffer, 0])[0]
            assert result == 0 and machine.mem_read(buffer, len(data)) == data
            observations.append({'parser': hex(0x95b3e4 + delta), 'path': path, 'result': result})

number_result = [101, 1000]
context = 0x1800000
suffix_buffer, prefix_buffer = buffer + 0x100, buffer + 0x110
machine.mem_write(prefix_buffer, b'DSCF')
machine.mem_write(0x10afae8, b'_FUJI')


def assembly(path):
    return '\n'.join(line.split('@', 1)[0] for line in (HERE / path).read_text().splitlines())


base = 0x3000000
assembler = Ks(KS_ARCH_ARM, KS_MODE_ARM)
date = assembly('xpro2_card_name.S') + '\n.ltorg\n'
helper = assembly('xpro2_date_folders.S')
folder_entry = base + len(assembler.asm(date, base)[0])
code = bytes(assembler.asm(date + helper, base)[0])
machine.mem_map(base, 0x1000)
machine.mem_write(base, code)
rtc = {'date': (26, 10, 22), 'status': 0, 'calls': 0}


def rtc_boundary(cpu, _pc, _size, _):
    rtc['calls'] += 1
    cpu.mem_write(cpu.reg_read(bench.REGS[0]), bytes((*rtc['date'], 12, 0, 0)))
    cpu.reg_write(bench.REGS[0], rtc['status'])
    cpu.reg_write(UC_ARM_REG_PC, cpu.reg_read(UC_ARM_REG_LR))


machine.hook_add(bench.u.UC_HOOK_CODE, rtc_boundary, begin=0xb30148, end=0xb30148)
for packed, status, expected_suffix, key in (
    ((26, 12, 31), 0, b'_1231\0', 0x20261231),
    ((27, 1, 1), 0, b'_0101\0', 0x20270101),
    ((26, 1, 1), 0, b'_0101\0', 0x20260101),
    ((24, 2, 29), 0, b'_0229\0', 0x20240229),
    ((0, 1, 1), 0, b'_0101\0', 0x20000101),
    ((107, 12, 31), 0, b'_1231\0', 0x21071231),
    ((100, 2, 29), 0, None, 0),
    ((26, 0, 1), 0, None, 0),
    ((26, 10, 22), 1, None, 0),
):
    rtc.update(date=packed, status=status, calls=0)
    sentinel = b'!.........?'
    machine.mem_write(suffix_buffer - 1, sentinel)
    regs = bench.call(machine, folder_entry, [suffix_buffer])
    expected = sentinel if expected_suffix is None else sentinel[:1] + expected_suffix + sentinel[7:]
    year, month, day = packed
    fat_date = 0 if expected_suffix is None else ((year + 20) << 9) | (month << 5) | day
    assert rtc['calls'] == 1 and regs[:3] == [int(expected_suffix is not None), key, fat_date]
    assert machine.mem_read(suffix_buffer - 1, len(sentinel)) == expected
    observations.append({'date': packed, 'rtcStatus': status, 'dateKey': hex(key), 'fatDate': fat_date,
                         'suffix': expected_suffix[:-1].decode() if expected_suffix is not None else None})

rtc.update(date=(26, 10, 22), status=0)
assert bench.call(machine, folder_entry, [suffix_buffer])[:2] == [1, 0x20261022]


def rtc_fields(cpu, _pc, _size, _):
    cpu.mem_write(cpu.reg_read(bench.REGS[0]), bytes((*rtc['date'], 12, 0, 0)))
    cpu.reg_write(UC_ARM_REG_PC, cpu.reg_read(UC_ARM_REG_LR))


# The directory initializer uses the stock ASCII calendar conversion below
# the RTC SDK. Substitute the clock fields only and compare the actual FAT date.
machine.hook_add(bench.u.UC_HOOK_CODE, rtc_fields, begin=0xb31384, end=0xb31384)
for delta in (0, 0x80000):
    for packed in ((24, 2, 29), (25, 10, 22), (26, 10, 22)):
        rtc.update(date=packed)
        _, bcd, fat_date, _ = bench.call(machine, folder_entry, [suffix_buffer])
        name = b'101' + bytes(machine.mem_read(suffix_buffer, 5)) + b'   '
        machine.mem_write(buffer, name + bytes(21))
        bench.call(machine, 0x96d50c + delta, [buffer])
        entry = bytes(machine.mem_read(buffer, 32))
        assert entry[:11] == name and entry[11] == 0x10
        dates = [struct.unpack_from('<H', entry, offset)[0] for offset in (16, 18, 24)]
        assert dates == [fat_date] * 3, (packed, dates, fat_date)
        observations.append({'directoryEntry': hex(0x96d50c + delta), 'dateKey': hex(bcd),
                             'name': name.rstrip().decode(), 'onDiskDateFields': dates})


def fixture_boundary(cpu, pc, _size, scan_entry):
    if pc == scan_entry:
        for register, value in zip(bench.REGS[:2], number_result):
            cpu.mem_write(cpu.reg_read(register), struct.pack('<H', value))
    cpu.reg_write(bench.REGS[0], 0)
    cpu.reg_write(UC_ARM_REG_PC, cpu.reg_read(UC_ARM_REG_LR))


for delta in (0, 0x80000):
    bench.word(machine, 0x9b3900 + delta, context)
    bench.word(machine, context + 0x20fc, 101)
    scan_entry = 0x956790 + delta
    for target in (0x967224 + delta, 0x95e4a0 + delta, scan_entry):
        machine.hook_add(bench.u.UC_HOOK_CODE, fixture_boundary, scan_entry, begin=target, end=target)
    for last_file, suffix, expected_folder, expected_file in (
        (1000, None, '101_FUJI', 'DSCF1001'),
        (1000, '_1022', '101_1022', 'DSCF1001'),
        (1999, '_1022', '102_1022', 'DSCF2001'),
    ):
        number_result[1] = last_file
        machine.mem_write(buffer, bytes(0xc4))
        result = bench.call(machine, 0x959144 + delta,
                            [buffer, suffix_buffer if suffix is not None else 0, prefix_buffer, 0])[0]
        path = bytes(machine.mem_read(buffer, 26)).rstrip(b'\0').decode('ascii')
        assert result == 0 and path == f'A:\\DCIM\\{expected_folder}\\{expected_file}', (result, path)
        assert machine.mem_read(buffer + 0x50, 8) == expected_folder.encode('ascii')
        observations.append({'constructor': hex(0x959144 + delta), 'suffix': suffix,
                             'scanResult': number_result.copy(), 'path': path, 'result': result})

# Run the scanner itself with a mounted DCF index and native suffix getters.
# Use a fresh machine so the synthetic scanner above cannot intercept this path.
machine = bench.machine(stock)
stored_suffix = 0xf4f8b0
situation = {'drive': 0, 'diagnostics': []}


def mounted_volume(cpu, pc, _size, boundaries):
    if pc == 0xba6458:
        situation['diagnostics'].append(tuple(cpu.reg_read(reg) for reg in bench.REGS[:2]))
    result = boundaries.get(pc, 0)
    cpu.reg_write(bench.REGS[0], result() if callable(result) else result)
    cpu.reg_write(UC_ARM_REG_PC, cpu.reg_read(UC_ARM_REG_LR))


def seed_index(delta, indexed, requested, *, file_number=1000, drive=0, maximum=101):
    situation['drive'] = drive
    situation['diagnostics'].clear()
    machine.mem_write(context, bytes(0x18000))
    machine.mem_write(context + 0x114, bytes([drive]))
    machine.mem_write(0x97b610 + delta, b'A\1\0\0B\1\1\0A')
    machine.mem_write(0x97d040 + delta, b'\0')
    machine.mem_write(0xf2b740, struct.pack('<HH', 101, 101))
    machine.mem_write(context + 0x2168 + 16 * drive, struct.pack('<HH', 101, file_number))
    bench.word(machine, context + 0x20fc, maximum)
    bench.word(machine, context + 4 * (0x966 + 101), 1)
    machine.mem_write(context + 0x16c00 + 5 * 101, indexed)
    machine.mem_write(stored_suffix, indexed)
    machine.mem_write(0x10afae8, requested)


machine.hook_add(bench.u.UC_HOOK_CODE, mounted_volume, {}, begin=0xba6458, end=0xba6458)
for delta in (0, 0x80000):
    boundaries = {0x967224 + delta: 0, 0x95e4a0 + delta: lambda: situation['drive'],
                  0x968c44 + delta: context}
    for target in boundaries:
        machine.hook_add(bench.u.UC_HOOK_CODE, mounted_volume, boundaries,
                         begin=target, end=target)
    bench.word(machine, 0x9b3900 + delta, context)
    machine.mem_write(prefix_buffer, b'DSCF')

    for indexed, requested, expected, flags in (
        (b'_FUJI', b'_FUJI', '101_FUJI\\DSCF1001', (0, 0)),
        (b'_1022', b'_1022', '101_1022\\DSCF1001', (0, 0)),
        (b'_1022', b'_1023', '102_1023\\DSCF2001', (1, 1)),
        (b'_FUJI', b'_1022', '102_1022\\DSCF2001', (1, 1)),
        (b'_1022', b'_FUJI', '102_FUJI\\DSCF2001', (1, 1)),
    ):
        # 1000 is a scan sentinel: the next file in folder 101 is DSCF1001.
        seed_index(delta, indexed, requested)
        machine.mem_write(buffer, bytes(0xc4))
        result = bench.call(machine, 0x959144 + delta, [buffer, 0, prefix_buffer, 0])[0]
        path = bytes(machine.mem_read(buffer, 26)).rstrip(b'\0').decode('ascii')
        actual_flags = tuple(machine.mem_read(buffer + offset, 1)[0] for offset in (0xbc, 0xbe))
        assert (result, path, actual_flags) == (0, 'A:\\DCIM\\' + expected, flags), (
            hex(delta), indexed, requested, result, path, actual_flags)
        observations.append({'scanner': hex(0x964624 + delta), 'indexed': indexed.decode(),
                             'requested': requested.decode(), 'path': path,
                             'createFlag': actual_flags[0], 'advanceFlag': actual_flags[1]})

    for maximum, result_code, folder in ((104, 0, '105_1023'), (999, 6, None)):
        seed_index(delta, b'_1022', b'_1023', drive=1, maximum=maximum)
        machine.mem_write(buffer, bytes(0xc4))
        result = bench.call(machine, 0x959144 + delta, [buffer, 0, prefix_buffer, 0])[0]
        assert result == result_code
        if folder is not None:
            path = machine.mem_read(buffer, 26).rstrip(b'\0')
            assert path == b'B:\\DCIM\\105_1023\\DSCF5001', path
            assert machine.mem_read(buffer + 0xbc, 3)[::2] == b'\1\1'
            assert not situation['diagnostics']
        else:
            assert machine.mem_read(buffer, 0xc4) == bytes(0xc4)
            assert situation['diagnostics'] == [(2, 0x51)]
        observations.append({'scanner': hex(0x964624 + delta), 'drive': 'B', 'maxFolder': maximum,
                             'result': result, 'folder': folder, 'diagnostics': situation['diagnostics'].copy()})
    # The entry reached after S2 in the full guest counts remaining file names.
    for target, result in ((0x964ee8 + delta, 1), (0x964f30 + delta, 0)):
        machine.hook_add(bench.u.UC_HOOK_CODE, mounted_volume, {target: result},
                         begin=target, end=target)
    seed_index(delta, b'_1022', b'_1022', file_number=1200)
    machine.mem_write(buffer, b'\0\0!')
    remaining = bench.call(machine, 0x958af4 + delta, [buffer])[0]
    assert machine.mem_read(buffer, 3) == struct.pack('<H', 101) + b'!'
    assert remaining == (999 - 101) * 999 + (999 - 200)
    observations.append({'capacityReader': hex(0x958af4 + delta),
                         'lastFolder': 101, 'lastFile': 1200, 'remainingNames': remaining})

print(json.dumps({'stockSha256': hashlib.sha256(stock).hexdigest(),
                  'scope': 'stock path, DCF index and FAT directory entry generation, no media operations',
                  'helperSourceSha256': hashlib.sha256((HERE / 'xpro2_date_folders.S').read_bytes()).hexdigest(),
                  'rtcFixture': 'SDK return and camera-local fields supplied by the test',
                  'constructorFixtures': ['ready volume', 'drive index zero', 'folder and file numbers from scanner'],
                  'scannerFixtures': ['ready volume', 'drive indices 0 and 1', 'mounted handle', 'synthetic DCF index'],
                  'capacityFixtures': ['acquired DCF lock', 'released DCF lock'],
                  'checks': len(observations), 'observations': observations}, indent=2))
