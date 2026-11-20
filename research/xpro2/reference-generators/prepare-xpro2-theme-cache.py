#!/usr/bin/env python3
"""Generate bounded native-command color cache helpers; no installed UI hooks."""
import os
from pathlib import Path
import hashlib
import json
import sys
from xpro2_patch_layout import check_module_allocation

ROOT = Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM

POOL, POOL_BYTES, CACHE, CACHE_END = 0x1d70670, 0x78000, 0x92000, 0xa6000
BASE, END, MAGIC, CAPTURED = 0x91000, 0x92000, 0x31434854, 0x4854
ENTRIES = {n: BASE+i*0x100 for i,n in enumerate(('slot','capture','commit','restore','reapply','refresh'))}


def read_color(register, pointer, offset, temporary):
    return f'ldrh {register},[{pointer},#{offset}]\nldrh {temporary},[{pointer},#{offset+2}]\norr {register},{register},{temporary},lsl #16\n'


def write_color(register, pointer, offset, temporary):
    return f'strh {register},[{pointer},#{offset}]\nlsr {temporary},{register},#16\nstrh {temporary},[{pointer},#{offset+2}]\n'


def main():
    stock = (ROOT.parent/'fujiextra2/build/ffboot_stock.bin').read_bytes()
    recipe = next(r for r in json.loads((ROOT.parent/'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json').read_text()) if r['firmware_id']=='xpro2-511')
    placements = [check_module_allocation(stock,recipe['plan'],a,b) for a,b in [(BASE,END),(CACHE,CACHE_END)]]
    # The allocator's own bound and stride, rather than a guessed record size.
    assert stock[0x21d87bc:0x21d87c0] == bytes.fromhex('c00080e2')
    assert int.from_bytes(stock[0x21d87e4:0x21d87e8],'little')+4 == POOL
    assert stock[0x21d6bb0:0x21d6bb4] == bytes.fromhex('130051e3')
    assert CACHE_END-CACHE == (POOL_BYTES//192)*32
    sources = {}
    sources['slot'] = f'''ldr r2,=0x{POOL:x}
subs r1,r0,r2
blo invalid
ldr r2,=0x{POOL_BYTES:x}
cmp r1,r2
bhs invalid
tst r1,#63
bne invalid
lsr r1,r1,#6
ldr r2,=0xaaaaaaab
umull ip,r3,r1,r2
lsr r3,r3,#1
add r2,r3,r3,lsl #1
cmp r1,r2
bne invalid
ldr r0,=0x{CACHE:x}
add r0,r0,r3,lsl #5
bx lr
invalid:
mov r0,#0
bx lr'''
    start = f'''push {{r4-r6,lr}}
mov r4,r0
mov r6,r1
bl 0x{ENTRIES['slot']:x}
cmp r0,#0
beq done
mov r5,r0
'''
    done = 'done:\npop {r4-r6,pc}\n'
    sources['capture'] = start+f'''mov r2,#0
str r2,[r5]
ldr r2,[r4,#8]
str r2,[r5,#4]
'''+read_color('r2','r4',0x46,'r3')+'str r2,[r5,#8]\n'+read_color('r2','r4',0x4a,'r3')+f'''str r2,[r5,#12]
ldrb r2,[r4,#0x11]
ldrb r3,[r4,#0x45]
orr r2,r2,r3,lsl #8
lsl r2,r2,#16
movw r3,#0x{CAPTURED:x}
orr r2,r2,r3
str r2,[r5,#28]
mov r0,r5
'''+done
    sources['commit'] = start+f'''cmp r6,#4
bhi invalid
ldr r2,[r5,#28]
uxth r3,r2
movw ip,#0x{CAPTURED:x}
cmp r3,ip
bne invalid
lsr r2,r2,#16
ldrb ip,[r4,#0x11]
ldrb r3,[r4,#0x45]
orr ip,ip,r3,lsl #8
cmp r2,ip
bne invalid
ldr r2,[r4,#8]
ldr r3,[r5,#4]
cmp r2,r3
bne invalid
'''+read_color('r2','r4',0x46,'r3')+'str r2,[r5,#16]\n'+read_color('r2','r4',0x4a,'r3')+f'''str r2,[r5,#20]
str r6,[r5,#24]
dmb sy
ldr r2,=0x{MAGIC:x}
str r2,[r5]
mov r0,r5
b done
invalid:
mov r0,#0
'''+done
    identity = f'''ldr r2,[r5]
ldr r3,=0x{MAGIC:x}
cmp r2,r3
bne invalid
ldr r2,[r4,#8]
ldr r3,[r5,#4]
cmp r2,r3
bne invalid
ldrb r2,[r4,#0x11]
ldrb r3,[r4,#0x45]
orr r2,r2,r3,lsl #8
ldr r3,[r5,#28]
lsr r3,r3,#16
cmp r2,r3
bne invalid
'''
    sources['restore'] = start+identity+read_color('r2','r4',0x46,'r3')+'''ldr r3,[r5,#16]
cmp r2,r3
bne invalid
'''+read_color('r2','r4',0x4a,'r3')+'''ldr r3,[r5,#20]
cmp r2,r3
bne invalid
ldr r2,[r5,#8]
'''+write_color('r2','r4',0x46,'r3')+'ldr r2,[r5,#12]\n'+write_color('r2','r4',0x4a,'r3')+'''mov r0,r5
b done
invalid:
mov r0,#0
'''+done
    sources['reapply'] = f'''push {{r4-r6,lr}}
mov r5,r0
mov r4,r1
cmp r5,#0
beq invalid
mov r0,r4
bl 0x{ENTRIES['slot']:x}
cmp r0,r5
bne invalid
'''+identity+read_color('r2','r4',0x46,'r3')+'''ldr r3,[r5,#8]
cmp r2,r3
bne invalid
'''+read_color('r2','r4',0x4a,'r3')+'''ldr r3,[r5,#12]
cmp r2,r3
bne invalid
ldr r2,[r5,#16]
'''+write_color('r2','r4',0x46,'r3')+'ldr r2,[r5,#20]\n'+write_color('r2','r4',0x4a,'r3')+'''mov r0,r5
b done
invalid:
mov r0,#0
'''+done
    # A theme change must force one native raster rebuild. Once the command
    # is committed under the new theme, normal SDK cache reuse resumes.
    sources['refresh'] = f'''ldr r1,=0x{CACHE:x}
subs r2,r0,r1
blo invalid
ldr r1,=0x{CACHE_END-CACHE:x}
cmp r2,r1
bhs invalid
tst r2,#31
bne invalid
ldr r1,[r0]
ldr r2,=0x{MAGIC:x}
cmp r1,r2
bne invalid
ldr r1,[r0,#24]
ldr r2,=0x90700
ldr r2,[r2]
cmp r2,#4
movhi r2,#0
cmp r1,r2
movne r0,#1
moveq r0,#0
bx lr
invalid:
mov r0,#0
bx lr'''
    blob = bytearray(END-BASE)
    out = ROOT/'analysis/xpro2-theme-cache-internal'
    out.mkdir(exist_ok=True)
    bodies = []
    for name, source in sources.items():
        address = ENTRIES[name]
        code = bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(source,address)[0])
        assert len(code)<=256, (name,len(code))
        blob[address-BASE:address-BASE+len(code)] = code
        (out/(name+'.s')).write_text(source+'\n')
        bodies.append({'name':name,'entry':hex(address),'bytes':len(code),'sha256':hashlib.sha256(code).hexdigest()})
    (out/'theme-cache.bin').write_bytes(blob)
    result = {'schema':1,'address':hex(BASE),'bytes':len(blob),'sha256':hashlib.sha256(blob).hexdigest(),
              'pool':hex(POOL),'pool_bytes':POOL_BYTES,'stride':192,'record_bytes':32,
              'cache':hex(CACHE),'cache_end':hex(CACHE_END),'entries':{k:hex(v) for k,v in ENTRIES.items()},
              'bodies':bodies,'placement':placements,
              'limitations':['Isolated helpers, no active firmware hooks',
                             'Pool covers groups0..19; unknown pointers are ignored',
                             'SDK compare wrappers and menu/persistence remain to connect']}
    (out/'provenance.json').write_text(json.dumps(result,indent=2)+'\n')
    print('PREPARED 6 bounded native command-cache helpers; no UI hook')


if __name__=='__main__':
    main()
