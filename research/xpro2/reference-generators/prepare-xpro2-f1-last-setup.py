#!/usr/bin/env python3
"""Reuse the last native SETUP folder as F1, preserving its connection settings."""
import os
from pathlib import Path
import argparse,hashlib,json,struct,sys
ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
from firmware_image import FirmwareImage
from xpro2_patch_layout import check_module_allocation
from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM
PAGE,SETUP,IQ=0xd3a828,0xd30610,0xd374e8
BASE,LABELS,ROOTLABELS,END=0xa8000,0xa8060,0xa8240,0xa8400
WRAPPER,WRAPPER_END,AVAIL=0xa8400,0xa8500,0x13d95e8

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--generic-page',action='store_true')
    args=parser.parse_args()
    f=FirmwareImage();stock=(ROOT.parent/'fujiextra2/build/ffboot_stock.bin').read_bytes()
    recipes=json.loads((ROOT.parent/'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json').read_text())
    recipe=next(r for r in recipes if r['firmware_id']=='xpro2-511')
    allocations=[check_module_allocation(stock,recipe['plan'],a,b) for a,b in [(BASE,END),(WRAPPER,WRAPPER_END)]]
    assert stock[BASE:WRAPPER_END]==bytes(WRAPPER_END-BASE)
    out=ROOT/('analysis/xpro2-f1-last-setup-generic-internal' if args.generic_page else 'analysis/xpro2-f1-last-setup-internal');out.mkdir(exist_ok=True)
    source=ROOT/'analysis/xpro2-theme-v3-integration-internal/theme-patches.json'
    original=json.loads(source.read_text())['patches']
    data=bytearray(END-BASE);fields=f.words(PAGE,7)
    assert fields[0]==0xc70448
    pairs=f.read(fields[4],7*8)
    assert struct.unpack_from('<2I',pairs,48)==(0x7ffffffe,0)
    new_pairs=pairs[:48]+struct.pack('<8I',0xf0,0x100500,0xf1,0x10051c,0xf2,0x1007d0,0x7ffffffe,0)
    data[:len(new_pairs)]=new_pairs
    rows=bytearray(f.read(fields[5],6*48))
    for i in range(6):struct.pack_into('<I',rows,i*48,0x7007)
    for caption in [0x7000,0x7001,0x7008]:
        row=bytearray(rows[:48]);struct.pack_into('<3I',row,0,0x7007,caption,caption);rows.extend(row)
    data[LABELS-BASE:LABELS-BASE+len(rows)]=rows
    setup_labels=bytearray(f.read(f.words(SETUP,7)[5],7*48))
    struct.pack_into('<I',setup_labels,6*48+4,0x7007)
    data[ROOTLABELS-BASE:ROOTLABELS-BASE+len(setup_labels)]=setup_labels
    path=out/'last-setup-data.bin';path.write_bytes(data)
    patches=[];changed=0;ready_changed=0;chain=None
    for p in original:
        a=int(p['address'],0)
        if a in (IQ+16,IQ+20):continue
        if a==AVAIL:
            word=int(p['value'],0);assert word>>24==0xea
            imm=word&0xffffff
            if imm&0x800000:imm-=0x1000000
            chain=a+8+4*imm
            continue
        if 'file' in p:
            blob=bytearray(Path(p['file']).read_bytes())
            descriptors=[d for d in [0x100500,0x10051c,0x1007d0] if a<=d and d+28<=a+len(blob)]
            if descriptors:
                for descriptor in descriptors:
                    at=descriptor-a;assert blob[at]==143
                    blob[at]=72;struct.pack_into('<I',blob,at+12,PAGE)
                q=out/f'native-f1-selectors-{a:x}.bin';q.write_bytes(blob);p={**p,'file':str(q)};changed+=len(descriptors)
            if a==0x102800:
                assembly=(ROOT/'analysis/xpro2-theme-preferences-v3-internal/service-ready.s').read_text()
                assert assembly.count('ldr r1,=0x100700')==1
                assembly=assembly.replace('ldr r1,=0x100700',f'ldr r1,=0x{PAGE:x}')
                code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(assembly,a)[0]);assert len(code)==len(blob)
                q=out/'service-ready-native-f1.bin';q.write_bytes(code)
                (out/'service-ready-native-f1.s').write_text(assembly)
                p={**p,'file':str(q)};ready_changed+=1
        patches.append(p)
    assert changed==3 and ready_changed==1 and chain is not None
    generic_available=f'''cmp r0,#6
bne connection_property
ldr ip,=0x1003980
ldr ip,[ip]
ldr r2,=0x{PAGE:x}
cmp ip,r2
bne fallback
cmp r1,#0xf0
blo original_row
cmp r1,#0xf2
bhi fallback
b available
original_row:
cmp r1,#7
bhi fallback
mov r0,#0xc7
b fallback
connection_property:
''' if args.generic_page else ''
    assembly=f'''push {{r1-r3,ip}}
{generic_available}
movw ip,#0xc7
cmp r0,ip
bne fallback
cmp r1,#0xf0
blo fallback
cmp r1,#0xf2
bhi fallback
available:
mov r0,#0
pop {{r1-r3,ip}}
bx lr
fallback:
pop {{r1-r3,ip}}
b 0x{chain:x}
'''
    code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(assembly,WRAPPER)[0])
    assert len(code)<=WRAPPER_END-WRAPPER
    q=out/'availability.bin';q.write_bytes(code);(out/'availability.s').write_text(assembly)
    delta=WRAPPER-AVAIL-8;assert delta%4==0 and -(1<<25)<=delta<(1<<25)
    patches += [{'address':hex(BASE),'file':str(path),'what':'Last native SETUP folder F1; six connection settings preserved plus three mod features'},
        {'address':hex(PAGE+16),'value':hex(BASE),'what':'Native F1 choices; original six plus three additions'},
        {'address':hex(PAGE+20),'value':hex(LABELS),'what':'Native F1 metadata and captions'},
        {'address':hex(SETUP+20),'value':hex(ROOTLABELS),'what':'Rename only last SETUP folder to F1'},
        {'address':hex(WRAPPER),'file':str(q),'what':'Availability for three added native F1 folder rows'},
        {'address':hex(AVAIL),'value':hex(0xea000000|((delta//4)&0xffffff)),'what':'Chain existing availability hook; preserve original rows'}]
    if args.generic_page:
        patches.append({'address':hex(PAGE),'value':'0x60448','what':'Use generic folder navigation for F1; preserve native group72 and UI registration'})
    spans=[]
    for p in patches:
        a=int(p['address'],0);n=Path(p['file']).stat().st_size if 'file' in p else 4
        assert all(a+n<=b or e<=a for b,e in spans),'Overlapping patches'
        spans.append((a,a+n))
    (out/'patches.json').write_text(json.dumps({'patches':patches},indent=2)+'\n')
    (out/'provenance.json').write_text(json.dumps({'schema':1,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),
        'page':hex(PAGE),'native_group':72,'setup_rows':7,'original_connection_rows':6,'added_rows':3,
        'allocations':allocations,'no_extra_sdk_adapters':True,'my_unchanged':True,'iq_rows':18,
        'availability_chain':hex(chain),'limitations':['Prepared candidate; full-machine validation separate']},indent=2)+'\n')
    print('PREPARED last native SETUP folder → F1;',len(patches),'patches')

if __name__=='__main__':main()
