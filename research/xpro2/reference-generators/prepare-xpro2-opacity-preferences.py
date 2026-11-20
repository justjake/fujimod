#!/usr/bin/env python3
"""Generate an internal asynchronous opacity preferences module, emulator only."""
import os
from pathlib import Path
import argparse
import copy
import hashlib
import json
import struct
import sys
from firmware_image import FirmwareImage
from xpro2_patch_layout import check_module_allocation

ROOT = Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM

BASE, END = 0x101000, 0x103000
STATE, READ, WRITE, SAVED, PATH = 0x102400, 0x102500, 0x102540, 0x102580, 0x102600
OPAC, HELPER = 0x103b00, 0x103800
ENTRIES = {name: BASE + i * 0x200 for i, name in enumerate(
    ('render', 'service', 'read-opened', 'read-done', 'read-closed',
     'write-opened', 'write-seeked', 'write-done', 'write-closed'))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed-graph', type=Path)
    parser.add_argument('--out', type=Path, default=ROOT / 'analysis/xpro2-opacity-preferences-internal')
    parser.add_argument('--menu-patches', type=Path, default=ROOT / 'analysis/xpro2-opacity-native-menu-summary-internal/menu-patches.json')
    args = parser.parse_args()
    out = args.out.resolve()
    if not out.is_relative_to(ROOT/'analysis'):
        raise ValueError('Internal output must remain inside analysis')
    out.mkdir(exist_ok=True)
    f = FirmwareImage()
    stock = (ROOT.parent / 'fujiextra2/build/ffboot_stock.bin').read_bytes()
    assert hashlib.sha256(stock).hexdigest() == 'ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172'
    assert stock[BASE:END] == bytes(END-BASE)
    recipe_path = ROOT.parent / 'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json'
    recipe = next(x for x in json.loads(recipe_path.read_text()) if x['firmware_id'] == 'xpro2-511')
    placement = check_module_allocation(stock, recipe['plan'], BASE, END)
    menu_provenance=json.loads((args.menu_patches.resolve().parent/'provenance.json').read_text())
    minimum_menu=(menu_provenance.get('f1') or {}).get('menu_minimum_percent',0)
    blocks = {}
    blocks['render'] = f'''push {{r4-r11,lr}}
sub sp,sp,#4
ldr ip,=0x{HELPER:x}
blx ip
push {{r0-r3}}
ldr ip,=0x{ENTRIES['service']:x}
blx ip
pop {{r0-r3}}
add sp,sp,#4
pop {{r4-r11,pc}}
'''
    blocks['service'] = f'''push {{r4-r8,lr}}
ldr r6,=0x{STATE:x}
add r8,r6,#64
ldrex r0,[r8]
cmp r0,#0
bne no_lock
mov r1,#1
strex r0,r1,[r8]
cmp r0,#0
bne no_lock
dmb sy
ldr r0,[r6,#8]
cmp r0,#4
beq ready
cmp r0,#0
bne done
ldr r0,=0x10a7fbc
ldrb r0,[r0]
cmp r0,#3
bne done
ldr r0,=0x10102a0
ldr r0,[r0]
ldr r1,=0x950400
cmp r0,r1
bne done
ldr r0,=0x9b4be4
ldrb r0,[r0]
cmp r0,#0x77
bne done
ldr r0,=0x9b4bec
ldrb r0,[r0]
cmp r0,#0x28
bne done
ldr ip,=0xba020c
blx ip
cmp r0,#0
beq done
str r0,[r6,#56]
ldr r4,=0x{OPAC:x}
ldr r5,=0x{SAVED:x}
ldmia r4,{{r0-r3}}
stmia r5,{{r0-r3}}
mov r0,#1
str r0,[r6,#8]
dmb sy
ldr r0,=0x{ENTRIES['read-opened']:x}
ldr r1,=0x{PATH:x}
mov r2,#0
ldr ip,=0xb9e86c
blx ip
b done
ready:
ldr r0,=0x10041bc
ldrb r0,[r0]
cmp r0,#0
bne done
ldr r4,=0x{OPAC:x}
ldmia r4,{{r0-r3}}
ldr ip,=0x4341504f
cmp r0,ip
bne invalid
cmp r1,#1
bne invalid
cmp r2,#100
cmpls r3,#100
bhi invalid
ldr r5,=0x{SAVED:x}
ldr r7,[r5,#8]
cmp r7,r2
ldreq r7,[r5,#12]
cmpeq r7,r3
beq done
ldr r5,=0x{WRITE:x}
stmia r5,{{r0-r3}}
mov r0,#0
str r0,[r6,#44]
mov r0,#5
str r0,[r6,#8]
dmb sy
ldr r0,=0x{ENTRIES['write-opened']:x}
ldr r1,=0x{PATH:x}
mov r2,#2
ldr ip,=0xb9e86c
blx ip
b done
invalid:
mov r0,#0x90
str r0,[r6,#44]
mov r0,#0x80
str r0,[r6,#8]
done:
dmb sy
mov r0,#0
str r0,[r8]
pop {{r4-r8,pc}}
no_lock:
clrex
pop {{r4-r8,pc}}
'''
    blocks['read-opened'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#12]
cmp r0,#0
bne default_ready
cmp r1,#0
beq default_ready
cmp r1,#0x10000000
bhs default_ready
str r1,[r4,#16]
mov r0,#2
str r0,[r4,#8]
dmb sy
ldr r0,=0x{ENTRIES['read-done']:x}
mov r2,#16
ldr r3,=0x{READ:x}
ldr ip,=0xb9e940
blx ip
pop {{r4-r6,pc}}
default_ready:
mov r0,#4
str r0,[r4,#8]
dmb sy
pop {{r4-r6,pc}}
'''
    blocks['read-done'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#20]
str r1,[r4,#24]
mov r0,#3
str r0,[r4,#8]
dmb sy
ldr r0,=0x{ENTRIES['read-closed']:x}
ldr r1,[r4,#16]
mov r2,#0
mov r3,#0
ldr ip,=0xb9e9e0
blx ip
pop {{r4-r6,pc}}
'''
    blocks['read-closed'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#28]
cmp r0,#0
bne load_error
ldr r0,[r4,#20]
cmp r0,#0
bne loaded
ldr r0,[r4,#24]
cmp r0,#16
bne loaded
ldr r5,=0x{READ:x}
ldmia r5,{{r0-r3}}
ldr ip,=0x4341504f
cmp r0,ip
bne loaded
cmp r1,#1
bne loaded
cmp r2,#100
cmpls r3,#100
bhi loaded
ldr r5,=0x{OPAC:x}
ldr r6,=0x{SAVED:x}
ldr ip,[r5,#8]
ldr r0,[r6,#8]
cmp ip,r0
ldreq ip,[r5,#12]
ldreq r0,[r6,#12]
cmpeq ip,r0
str r2,[r6,#8]
str r3,[r6,#12]
str r2,[r5,#8]
str r3,[r5,#12]
loaded:
ldr r0,[r4,#48]
add r0,r0,#1
str r0,[r4,#48]
mov r0,#4
dmb sy
str r0,[r4,#8]
pop {{r4-r6,pc}}
load_error:
mov r0,#0x91
str r0,[r4,#44]
mov r0,#0x80
str r0,[r4,#8]
pop {{r4-r6,pc}}
'''
    # Conditional loads above compare live values with their initial baseline.
    # Apply a loaded record only if the user has not changed them meanwhile.
    blocks['read-closed'] = blocks['read-closed'].replace('str r2,[r5,#8]\nstr r3,[r5,#12]',
                                                        'streq r2,[r5,#8]\nstreq r3,[r5,#12]')
    blocks['write-opened'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#60]
cmp r0,#0
bne write_open_error
cmp r1,#0
beq write_open_error
cmp r1,#0x10000000
bhs write_open_error
str r1,[r4,#16]
mov r0,#6
str r0,[r4,#8]
dmb sy
ldr r0,=0x{ENTRIES['write-seeked']:x}
mov r2,#0
mov r3,#0
sub sp,sp,#8
mov ip,#0
str ip,[sp]
ldr ip,=0xb9ea90
blx ip
add sp,sp,#8
pop {{r4-r6,pc}}
write_open_error:
mov r0,#0x92
str r0,[r4,#44]
mov r0,#0x80
str r0,[r4,#8]
pop {{r4-r6,pc}}
'''
    blocks['write-seeked'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#40]
cmp r0,#0
bne seek_error
mov r0,#7
str r0,[r4,#8]
dmb sy
ldr r0,=0x{ENTRIES['write-done']:x}
ldr r1,[r4,#16]
mov r2,#16
ldr r3,=0x{WRITE:x}
ldr ip,=0xb9e990
blx ip
pop {{r4-r6,pc}}
seek_error:
mov r0,#0x93
str r0,[r4,#44]
b close_write
close_write:
mov r0,#8
str r0,[r4,#8]
ldr r0,=0x{ENTRIES['write-closed']:x}
ldr r1,[r4,#16]
mov r2,#0
mov r3,#0
ldr ip,=0xb9e9e0
blx ip
pop {{r4-r6,pc}}
'''
    blocks['write-done'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#32]
str r1,[r4,#36]
cmp r0,#0
cmpeq r1,#16
movne r0,#0x94
strne r0,[r4,#44]
mov r0,#8
str r0,[r4,#8]
dmb sy
ldr r0,=0x{ENTRIES['write-closed']:x}
ldr r1,[r4,#16]
mov r2,#0
mov r3,#0
ldr ip,=0xb9e9e0
blx ip
pop {{r4-r6,pc}}
'''
    blocks['write-closed'] = f'''push {{r4-r6,lr}}
ldr r4,=0x{STATE:x}
str r0,[r4,#28]
cmp r0,#0
movne r0,#0x95
strne r0,[r4,#44]
ldr r0,[r4,#44]
cmp r0,#0
bne save_error
ldr r5,=0x{WRITE:x}
ldr r6,=0x{SAVED:x}
ldmia r5,{{r0-r3}}
stmia r6,{{r0-r3}}
ldr r0,[r4,#52]
add r0,r0,#1
str r0,[r4,#52]
mov r0,#4
dmb sy
str r0,[r4,#8]
pop {{r4-r6,pc}}
save_error:
mov r0,#0x80
dmb sy
str r0,[r4,#8]
pop {{r4-r6,pc}}
'''
    if minimum_menu:
        for name,label in [('service','invalid'),('read-closed','loaded')]:
            blocks[name]=blocks[name].replace('cmp r2,#100',f'cmp r2,#{minimum_menu}\nblo {label}\ncmp r2,#100')
    patches, provenance = [], []
    for name, source in blocks.items():
        address = ENTRIES[name]
        data = bytes(Ks(KS_ARCH_ARM, KS_MODE_ARM).asm(source, address)[0])
        assert len(data) <= 0x200, (name, len(data))
        binary = out / (name + '.bin')
        binary.write_bytes(data)
        (out / (name + '.s')).write_text(source)
        patches.append({'address': hex(address), 'file': str(binary), 'what': 'Internal opacity preferences module'})
        provenance.append({'name': name, 'address': hex(address), 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
    path_bytes = (ROOT / 'analysis/xpro2-opacity-sd-seek-internal/path.bin').read_bytes()
    for name, address, data in [('state', STATE, struct.pack('<18I', 0x46455250, 1, *([0]*16))),
                                ('read-buffer', READ, bytes([0xcc])*16), ('write-buffer', WRITE, bytes(16)),
                                ('baseline', SAVED, bytes(16)), ('path', PATH, path_bytes)]:
        assert address % 4 == 0 and len(data) % 4 == 0
        binary = out / (name + '.bin')
        binary.write_bytes(data)
        patches.append({'address': hex(address), 'file': str(binary), 'what': 'Persistent async I/O buffer, not stack storage'})
    menu = args.menu_patches.resolve()
    original = json.loads(menu.read_text())['patches']
    sites = [int(p['address'], 0) for p in original if p.get('what', '').startswith('BL opacity helper')]
    assert len(sites) == 9
    combined = [p for p in original if int(p['address'], 0) not in sites]
    for site in sites:
        instruction = f.disasm(site, 4)[0]
        assert instruction.mnemonic == 'bl' and instruction.op_str == '#0xb5accc'
        delta = ENTRIES['render'] - site - 8
        assert delta % 4 == 0 and -(1 << 25) <= delta < (1 << 25)
        patches.append({'address': hex(site), 'value': hex(0xeb000000 | ((delta // 4) & 0xffffff)),
                        'what': 'Opacity helper once, then nonblocking preferences service'})
    (out / 'preferences-patches.json').write_text(json.dumps({'patches': combined + patches}, indent=2) + '\n')
    report = {'schema': 1, 'allocation': [hex(BASE), hex(END)], 'placement': placement,
              'state': hex(STATE), 'read_buffer': hex(READ), 'write_buffer': hex(WRITE),
              'baseline': hex(SAVED), 'opacity_state': hex(OPAC), 'path': hex(PATH),
              'minimum_menu_percent':minimum_menu,
              'blocks': provenance, 'menu_source_sha256': hashlib.sha256(menu.read_bytes()).hexdigest(),
              'limitations': ['Internal unqualified integration', 'No runtime or fresh-boot persistence claim']}
    (out / 'provenance.json').write_text(json.dumps(report, indent=2) + '\n')
    if args.seed_graph:
        document = copy.deepcopy(json.loads(args.seed_graph.read_text()))
        graph = document['graph']
        node = next(x for x in graph['devices']['nodes'] if x['name'] == 'sd')
        assert '/OPACITY.BIN' not in node['model']['files']
        node['model']['files']['/OPACITY.BIN'] = 'opacity-preferences-seed'
        graph['assets']['opacity-preferences-seed'] = {'kind': 'bytes', 'bytes': list(struct.pack('<4I', 0x4341504f, 1, 25, 75))}
        (out / 'seeded-graph.json').write_text(json.dumps(document, indent=2) + '\n')
    print('PREPARED', len(combined + patches), 'internal preferences patches')


if __name__ == '__main__':
    main()
