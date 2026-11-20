#!/usr/bin/env python3
"""Prototype of FujiMod's opacity scaling on the original X-Pro2 layer API.

Fixed percentage or native menu/HUD selection; settings UI/persistence pending.
Generates declared SD/RAM patches; never rewrites the official DAT.
"""
import os
from pathlib import Path
import argparse,hashlib,json,struct,sys
import unicorn
from firmware_image import FirmwareImage
ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
BASE=0x103800
API=0xb5accc
STATE=0x103b00
MAGIC=0x4341504f # little endian OPAC

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--percent',type=int,choices=[0,25,50,75,100])
    parser.add_argument('--menu-percent',type=int,choices=[0,25,50,75,100])
    parser.add_argument('--hud-percent',type=int,choices=[0,25,50,75,100])
    args=parser.parse_args()
    split=args.menu_percent is not None or args.hud_percent is not None
    if split and args.percent is not None:parser.error('--percent cannot be combined with menu/HUD percentages')
    percent=50 if args.percent is None else args.percent
    menu=50 if args.menu_percent is None else args.menu_percent
    hud=100 if args.hud_percent is None else args.hud_percent
    from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM
    select=f'ldrb r0,[sp,#10]\nmov r1,#{percent}\n'
    if split:
        select=f'''ldr r3,=0x10a7fbc
ldrb r3,[r3]
cmp r3,#3
bne original
ldr r2,=0x{STATE:x}
ldr r0,[r2]
ldr r3,=0x{MAGIC:x}
cmp r0,r3
bne original
ldr r0,[r2,#4]
cmp r0,#1
bne original
ldr r3,=0x10041bc
ldrb r3,[r3]
cmp r3,#0
ldreq r1,[r2,#12]
beq scale
ldr r1,[r2,#8]
ldr r3,=0xf4caf7
ldrb r3,[r3]
cmp r3,#1
moveq r2,#0
ldrne r2,=0x474
ldr r3,=0xf4e05c
ldrb r3,[r3,r2]
tst r3,#1
beq original
scale:
cmp r1,#100
bhi original
ldrb r0,[sp,#10]
'''
    source=f'''push {{r4,r5,r6,lr}}
sub sp,sp,#16
mov r4,r0
mov r5,r1
mov r6,r2
ldrh r0,[r6,#0]
strh r0,[sp,#0]
ldrh r0,[r6,#2]
strh r0,[sp,#2]
ldrh r0,[r6,#4]
strh r0,[sp,#4]
ldrh r0,[r6,#6]
strh r0,[sp,#6]
ldrh r0,[r6,#8]
strh r0,[sp,#8]
ldrh r0,[r6,#10]
strh r0,[sp,#10]
cmp r4,#1
cmpeq r5,#0
bne original
{select}\
mul r0,r1,r0
add r0,r0,#50
ldr r1,=0x51eb851f
umull r2,r3,r0,r1
lsr r0,r3,#5
strb r0,[sp,#10]
original:
mov r0,r4
mov r1,r5
mov r2,sp
ldr r3,=0x{API:x}
blx r3
add sp,sp,#16
pop {{r4,r5,r6,pc}}
'''
    encoded,_=Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(source,BASE)
    body=bytes(encoded);assert len(body)%4==0 and BASE+len(body)<=(STATE if split else 0x104000)
    f=FirmwareImage();stock=(ROOT.parent/'fujiextra2/build/ffboot_stock.bin').read_bytes()
    assert hashlib.sha256(stock).hexdigest()=='ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172'
    assert stock[BASE:BASE+len(body)]==bytes(len(body))
    label=f'menu{menu}-hud{hud}' if split else str(percent)
    out=ROOT/f'analysis/xpro2-opacity-{label}-prototype';out.mkdir(exist_ok=True)
    helper=out/'opacity-helper.bin';helper.write_bytes(body)
    (out/'opacity-helper.s').write_text(source)
    edits=[{'address':hex(BASE),'file':str(helper),'what':'New bounded opacity helper in the measured SD stub/table gap'}]
    if split:
        assert stock[STATE:STATE+16]==bytes(16)
        state_file=out/'opacity-state.bin';state_file.write_bytes(struct.pack('<4I',MAGIC,1,menu,hud))
        edits.append({'address':hex(STATE),'file':str(state_file),
                      'what':'Native opacity state: OPAC magic, schema1, menu percent, HUD percent. No persistence yet.'})
    callers=[]
    for call in f.direct_calls_to([API]):
        if call['region'] not in ['section_5','section_6']:continue
        address=int(call['source'],16);old=f.read(address,4)
        assert stock[address:address+4]==old and struct.unpack('<I',old)[0]>>24==0xeb
        delta=BASE-address-8;assert delta%4==0 and -(1<<25)<=delta<(1<<25)
        word=0xeb000000|((delta//4)&0xffffff)
        edits.append({'address':hex(address),'value':hex(word),'what':f'BL opacity helper, original target {API:#x}'})
        callers.append({'address':hex(address),'expected_old_word':hex(struct.unpack('<I',old)[0]),'new_word':hex(word)})
    assert callers,'No validated original callers'
    patch=out/'opacity-patches.json';patch.write_text(json.dumps({'name':f'XPro2 opacity {label} prototype',
      'commento':'Native menu/HUD opacity selection; settings UI and persistence pending.' if split else 'Fixed OSD layer0 opacity; emulator prototype only.',
      'patches':edits},indent=2)+'\n')
    (out/'provenance.json').write_text(json.dumps({'percent':None if split else percent,'helper_address':hex(BASE),
      'helper_size':len(body),'helper_sha256':hashlib.sha256(body).hexdigest(),
      'original_api':hex(API),'callers':callers,'stock_sha256':hashlib.sha256(stock).hexdigest(),
      'reference_formula':'(alpha * percent + 50) / 100, from X70 0.0.1 function 0x3eac50',
      'runtime_state':{'address':hex(STATE),'bytes':16,'menu_percent':menu,'hud_percent':hud,
        'menu_active':{'address':'0x10041bc','width':1,'getter':'0xa52058'},
        'system_mode':{'address':'0x10a7fbc','width':1,'record_mode':3},
        'display_select':{'address':'0xf4caf7','width':1,'bank_rule':'0 if value == 1 else 1'},
        'display_enable':{'address':'0xf4e05c','bank_stride':'0x474','mask':1}} if split else None,
      'limitations':['UI settings and persistence not ported',
        'Display/ABI validation must match the exact helper digest',
        'Free gap checked statically, not a hardware safety proof']},indent=2)+'\n')
    print(json.dumps({'patch':str(patch),'helper_bytes':len(body),'callers':len(callers)}))
if __name__=='__main__':main()
