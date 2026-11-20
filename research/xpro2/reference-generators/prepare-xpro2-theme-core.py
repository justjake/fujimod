#!/usr/bin/env python3
"""Relocate shipped theme color filters into checked X-Pro2 RAM; no UI hook."""
import os
from pathlib import Path
import hashlib
import json
import struct
import sys
import capstone
from capstone.arm import ARM_OP_MEM, ARM_REG_PC
from xpro2_patch_layout import check_module_allocation

ROOT = Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM

BASE, END, STORE, WRAPPER, STATE = 0x90000, 0x91000, 0x90400, 0x90500, 0x90700
ORIGIN, LIMIT = 0x3e7fbc, 0x3e83ac


def immediate(word):
    return ((word >> 4) & 0xf000) | (word & 0xfff)


def replace_immediate(word, value):
    return (word & ~0x000f0fff) | ((value & 0xf000) << 4) | (value & 0xfff)


def main():
    source = ROOT / 'analysis/fujimod-release-0.0.1/mod-region-00390000.bin'
    raw = source.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == 'c7413cecadedb8cf66282d0b403af6f6395c72dae1136ceb31266112867dc200'
    stock = (ROOT.parent / 'fujiextra2/build/ffboot_stock.bin').read_bytes()
    recipe = next(r for r in json.loads((ROOT.parent / 'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json').read_text()) if r['firmware_id'] == 'xpro2-511')
    placement = check_module_allocation(stock, recipe['plan'], BASE, END)
    code = bytearray(raw[ORIGIN-0x390000:LIMIT-0x390000])
    cs = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_ARM)
    cs.detail = True
    changes = []
    for instruction in cs.disasm(bytes(code), ORIGIN):
        assert not any(op.type == ARM_OP_MEM and op.mem.base == ARM_REG_PC for op in instruction.operands), 'Unexpected PC-relative data'
        at = instruction.address-ORIGIN
        word = struct.unpack_from('<I', code, at)[0]
        if word & 0x0ff00000 == 0x03400000 and immediate(word) == 0x404d:
            previous = struct.unpack_from('<I', code, at-4)[0]
            assert previous & 0x0ff00000 == 0x03000000 and previous >> 12 & 15 == word >> 12 & 15
            old = (0x404d << 16) | immediate(previous)
            new = {0x404dc438: STATE+4, 0x404dc43c: STATE+8}[old]
            struct.pack_into('<I', code, at-4, replace_immediate(previous, new & 0xffff))
            struct.pack_into('<I', code, at, replace_immediate(word, new >> 16))
            changes.append({'kind': 'owned_state', 'pc': hex(instruction.address-4), 'old': hex(old), 'new': hex(new)})
        elif instruction.mnemonic == 'bl':
            assert instruction.operands[0].imm == 0x3e7ee0, instruction.op_str
            delta = STORE-(BASE+at)-8
            assert delta % 4 == 0 and -(1 << 25) <= delta < (1 << 25)
            struct.pack_into('<I', code, at, (word & 0xff000000) | ((delta//4) & 0xffffff))
            changes.append({'kind': 'color_store', 'pc': hex(instruction.address), 'new': hex(STORE)})
    assert sum(c['kind'] == 'color_store' for c in changes) == 3
    # For this isolated RGB adapter, the original color store's pool-cache
    # lookup always returns -1 (scratch command outside the drawing pool).
    # Preserve its foreground/background semantics without a foreign cache.
    store = '''ldrb ip,[r0,#0x5e]
strb r1,[r0,#0x57]
strb r2,[r0,#0x58]
strb r3,[r0,#0x59]
cmp ip,#0
ldrbne r1,[sp]
ldrbne r2,[sp,#4]
ldrbne r3,[sp,#8]
strb r1,[r0,#0x5b]
strb r2,[r0,#0x5c]
strb r3,[r0,#0x5d]
bx lr'''
    # Entry+8 accepts an explicit theme in r2, so two cores can shade commands
    # using their own captured selection without temporarily changing globals.
    wrapper = f'''ldr r2,=0x{STATE:x}
ldr r2,[r2]
push {{r4-r7,lr}}
sub sp,sp,#220
mov r4,r0
mov r5,r1
mov r6,r2
mov r7,sp
mov ip,sp
mov r2,#53
mov r3,#0
clear:
str r3,[ip],#4
subs r2,r2,#1
bne clear
mov r3,#1
str r3,[r7,#0x14]
lsr r3,r4,#16
strb r3,[r7,#0x57]
lsr r3,r4,#8
strb r3,[r7,#0x58]
strb r4,[r7,#0x59]
cmp r6,#1
bne filters
ldr r3,=0x949494
cmp r4,r3
bne output
mov r3,#255
strb r3,[r7,#0x57]
strb r3,[r7,#0x58]
strb r3,[r7,#0x59]
b output
filters:
cmp r6,#2
ldreq ip,=0x{BASE:x}
beq invoke
cmp r6,#3
ldreq ip,=0x{BASE+0x3e8150-ORIGIN:x}
beq invoke
cmp r6,#4
ldrne ip,=0
bne output
ldr ip,=0x{BASE+0x3e8254-ORIGIN:x}
invoke:
mov r0,r7
blx ip
output:
ldrb r0,[r7,#0x57]
strb r0,[r5]
ldrb r0,[r7,#0x58]
strb r0,[r5,#1]
ldrb r0,[r7,#0x59]
strb r0,[r5,#2]
add sp,sp,#220
pop {{r4-r7,pc}}'''
    ks = Ks(KS_ARCH_ARM, KS_MODE_ARM)
    blob = bytearray(END-BASE)
    blob[:len(code)] = code
    for address, text, limit in [(STORE, store, WRAPPER), (WRAPPER, wrapper, STATE)]:
        body = bytes(ks.asm(text, address)[0])
        assert address+len(body) <= limit
        blob[address-BASE:address-BASE+len(body)] = body
    out = ROOT / 'analysis/xpro2-theme-core-internal'
    out.mkdir(exist_ok=True)
    binary = out / 'theme-core.bin'
    binary.write_bytes(blob)
    (out / 'color-store.s').write_text(store+'\n')
    (out / 'rgb-adapter.s').write_text(wrapper+'\n')
    (out / 'provenance.json').write_text(json.dumps({'schema': 1,
        'source_sha256': hashlib.sha256(raw).hexdigest(), 'address': hex(BASE), 'end': hex(END),
        'entry': hex(WRAPPER), 'snapshot_entry':hex(WRAPPER+8), 'state': hex(STATE), 'sha256': hashlib.sha256(blob).hexdigest(),
        'placement': placement, 'relocations': changes,
        'limitations': ['Isolated RGB adapter only, not hooked into the camera UI',
                        'Native command layout, contextual colors and cache handling remain to port']}, indent=2)+'\n')
    print('PREPARED relocated theme RGB core;', len(changes), 'checked relocations; no UI hook')


if __name__ == '__main__':
    main()
