#!/usr/bin/env python3
"""Prepare an internal menu-render adapter; never update the owner profile."""
import os
from pathlib import Path
import hashlib
import json
import struct
import sys
from firmware_image import FirmwareImage

ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM

ENTRY,PALETTE,TABLE,QUEUE=0x91c00,0x91e40,0x91f00,0x21d8734


def main():
    compare=ROOT/'analysis/xpro2-theme-compare-internal'
    p=json.loads((compare/'provenance.json').read_text())
    core=ROOT/'analysis/xpro2-theme-core-internal'
    cp=json.loads((core/'provenance.json').read_text())
    cache=json.loads((ROOT/'analysis/xpro2-theme-cache-internal/provenance.json').read_text())
    blob=bytearray((compare/'theme-compare.bin').read_bytes())
    assert hashlib.sha256(blob).hexdigest()==p['sha256']
    f=FirmwareImage()
    original=f.read(QUEUE,8)
    assert original==bytes.fromhex('04b02de500b08de2')
    source=(ROOT/'analysis/fujimod-release-0.0.1/mod-region-00390000.bin').read_bytes()
    assert hashlib.sha256(source).hexdigest()==cp['source_sha256']
    palettes=source[0x438a7c-0x390000:0x438a7c-0x390000+5*7*4]
    words=struct.unpack('<2I',original)
    body=f'''push {{r0,r1}}
ldr r0,=0x10041bc
ldrb r0,[r0]
cmp r0,#0
pop {{r0,r1}}
beq original
push {{r0-r12,lr}}
sub sp,sp,#16
mov r4,r0
mov r5,r1
cmp r4,#19
bhi finish
ldrb r0,[r5,#0x11]
cmp r0,#1
cmpne r0,#5
bne finish
ldr r6,=0x{int(cp['state'],16):x}
ldr r6,[r6]
cmp r6,#4
movhi r6,#0
mov r0,r5
bl {cache['entries']['capture']}
cmp r0,#0
beq finish
cmp r6,#0
beq commit
ldrb r0,[r5,#0x46]
lsl r0,r0,#16
ldrb r1,[r5,#0x47]
orr r0,r0,r1,lsl #8
ldrb r1,[r5,#0x48]
orr r0,r0,r1
ldrb r1,[r5,#0x11]
cmp r1,#5
beq shape
ldr r1,=0xffffff
cmp r0,r1
ldreq r0,=0x949494
ldr r1,=0x5a5a5a
cmp r0,r1
ldreq r0,=0x5a5a5e
mov r1,sp
mov r2,r6
bl {cp['snapshot_entry']}
ldrb r0,[sp]
strb r0,[r5,#0x46]
ldrb r0,[sp,#1]
strb r0,[r5,#0x47]
ldrb r0,[sp,#2]
strb r0,[r5,#0x48]
ldrb r0,[r5,#0x4d]
cmp r0,#0
beq commit
mov r1,#28
mul r0,r6,r1
ldr r1,=0x{TABLE+4:x}
ldr r0,[r1,r0]
lsr r1,r0,#16
strb r1,[r5,#0x4a]
lsr r1,r0,#8
strb r1,[r5,#0x4b]
strb r0,[r5,#0x4c]
b commit
shape:
ldr r1,=0xffffff
cmp r0,r1
bne palette
ldrb r1,[r5,#0x49]
cmp r1,#255
ldrlo r0,=0xb91000
palette:
mov r1,r6
bl 0x{PALETTE:x}
lsr r1,r0,#16
strb r1,[r5,#0x46]
lsr r1,r0,#8
strb r1,[r5,#0x47]
strb r0,[r5,#0x48]
commit:
mov r0,r5
mov r1,r6
bl {cache['entries']['commit']}
finish:
add sp,sp,#16
pop {{r0-r12,lr}}
original:
.word 0x{words[0]:x}
.word 0x{words[1]:x}
ldr pc,[pc,#-4]
.word 0x{QUEUE+8:x}'''
    palette=f'''cmp r1,#0
bxeq lr
cmp r1,#4
bxhi lr
cmp r0,#0
ldreq r0,=0x262626
ldr r2,=0x1e1e1e
cmp r0,r2
ldreq r0,=0x262626
ldr r2,=0x{TABLE:x}
mov ip,#0
next:
ldr r3,[r2,ip,lsl #2]
cmp r0,r3
beq found
add ip,ip,#1
cmp ip,#7
blo next
bx lr
found:
mov r3,#28
mul r1,r3,r1
add r1,r1,ip,lsl #2
ldr r0,[r2,r1]
bx lr'''
    output=ROOT/'analysis/xpro2-theme-render-internal'
    output.mkdir(exist_ok=True)
    for address,text,limit,name in [(ENTRY,body,PALETTE,'enqueue'),(PALETTE,palette,TABLE,'palette')]:
        code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(text,address)[0])
        assert address+len(code)<=limit,(name,len(code))
        blob[address-0x91000:address-0x91000+len(code)]=code
        (output/(name+'.s')).write_text(text+'\n')
    blob[TABLE-0x91000:TABLE-0x91000+len(palettes)]=palettes
    binary=output/'theme-render.bin'
    binary.write_bytes(blob)
    patches=[{'address':'0x90000','file':str(core/'theme-core.bin'),'what':'Bounded theme RGB core, state and menu data space'},
             {'address':'0x91000','file':str(binary),'what':'Native theme command cache, SDK comparisons and enqueue adapter'}]
    hooks=[{'target':hex(QUEUE),'entry':hex(ENTRY),'expected':original.hex()}]
    for block in p['blocks']:
        hooks.append({'target':block['start'],'entry':block['entry'],
                      'expected':f.read(int(block['start'],16),8).hex()})
    for hook in hooks:
        address=int(hook['target'],16)
        patches.extend([{'address':hex(address),'value':'0xe51ff004','what':'Checked two-word absolute ARM bridge'},
                        {'address':hex(address+4),'value':hook['entry'],'what':'Owned bridge destination'}])
    (output/'theme-patches.json').write_text(json.dumps({'patches':patches},indent=2)+'\n')
    result={'schema':1,'address':'0x91000','entry':hex(ENTRY),'palette_entry':hex(PALETTE),
            'palette_table':hex(TABLE),'sha256':hashlib.sha256(blob).hexdigest(),
            'core_sha256':cp['sha256'],'cache_sha256':cache['sha256'],'compare_sha256':p['sha256'],
            'hooks':hooks,'native_layout':'192-byte commands, foreground46..48/background4a..4c',
            'limitations':['Internal unqualified prototype; no owner profile change',
                           'Contextual styling and native bitmap/resource handling remain under review',
                           'F1 selector, persistence and complete live-render qualification pending']}
    (output/'provenance.json').write_text(json.dumps(result,indent=2)+'\n')
    print('PREPARED internal native theme-render adapter and checked absolute bridges')


if __name__=='__main__':main()
