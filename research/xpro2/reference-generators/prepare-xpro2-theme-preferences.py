#!/usr/bin/env python3
"""Extend the verified opacity preferences with a backward-compatible theme word."""
import os
from pathlib import Path
import argparse
import hashlib
import json
import sys

ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM


def replace_once(text,old,new):
    assert text.count(old)==1,old
    return text.replace(old,new)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out',type=Path,default=ROOT/'analysis/xpro2-theme-preferences-v2-internal')
    parser.add_argument('--save-confirmed-f1',action='store_true',
                        help='Allow saving on the F1 parent page after a selector closes')
    args=parser.parse_args()
    source=ROOT/'analysis/xpro2-f1-theme-preferences-internal'
    out=args.out.resolve()
    assert out.is_relative_to(ROOT/'analysis')
    out.mkdir(exist_ok=True)
    p=json.loads((source/'provenance.json').read_text())
    addresses={b['name']:int(b['address'],16) for b in p['blocks']}
    s,r,w,b,o=[int(p[k],16) for k in ('state','read_buffer','write_buffer','baseline','opacity_state')]
    theme=0x90700
    addresses.update({'service-ready':0x102800,'read-tail':0x102a00})
    sources={name:(source/(name+'.s')).read_text() for name in addresses if name not in ('service-ready','read-tail')}
    service=sources['service']
    prefix,ready=service.split('ready:\n')
    prefix=replace_once(prefix,'stmia r5,{r0-r3}\n',f'stmia r5,{{r0-r3}}\nldr r0,=0x{theme:x}\nldr r0,[r0]\nstr r0,[r5,#16]\n')
    sources['service']=prefix+f'ready:\nb 0x{addresses["service-ready"]:x}\n'+'done:\n'+ready.split('done:\n',1)[1]
    menu_guard='''bne done'''
    if args.save_confirmed_f1:
        menu_guard='''beq values
ldr r0,=0x1003980
ldr r0,[r0]
ldr r1,=0x100700
cmp r0,r1
bne done
values:'''
    sources['service-ready']=f'''ldr r0,=0x10041bc
ldrb r0,[r0]
cmp r0,#0
{menu_guard}
ldr r4,=0x{o:x}
ldmia r4,{{r0-r3}}
ldr ip,=0x4341504f
cmp r0,ip
bne invalid
cmp r1,#1
bne invalid
cmp r2,#{p['minimum_menu_percent']}
blo invalid
cmp r2,#100
cmpls r3,#100
bhi invalid
ldr r7,=0x{theme:x}
ldr r7,[r7]
cmp r7,#4
bhi invalid
ldr r5,=0x{b:x}
ldr ip,[r5,#8]
cmp ip,r2
ldreq ip,[r5,#12]
cmpeq ip,r3
ldreq ip,[r5,#16]
cmpeq ip,r7
beq done
ldr r5,=0x{w:x}
stmia r5,{{r0-r3}}
mov r0,#2
str r0,[r5,#4]
str r7,[r5,#16]
mov r0,#0
str r0,[r6,#44]
mov r0,#5
str r0,[r6,#8]
dmb sy
ldr r0,=0x{addresses['write-opened']:x}
ldr r1,=0x{int(p['path'],16):x}
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
'''
    # Keep the original first READ16 unchanged. Only schema2 requests a
    # second READ4, avoiding dependence on an SDK short-read/EOF convention.
    sources['read-done']=f'''push {{r4-r6,lr}}
ldr r4,=0x{s:x}
str r0,[r4,#20]
str r1,[r4,#24]
cmp r0,#0
cmpeq r1,#16
bne close
ldr r5,=0x{r:x}
ldr r2,[r5]
ldr r3,=0x4341504f
cmp r2,r3
ldreq r2,[r5,#4]
cmpeq r2,#2
bne close
ldr r0,=0x{addresses['read-tail']:x}
ldr r1,[r4,#16]
mov r2,#4
ldr r3,=0x{r+16:x}
ldr ip,=0xb9e940
blx ip
pop {{r4-r6,pc}}
close:
mov r0,#3
str r0,[r4,#8]
dmb sy
ldr r0,=0x{addresses['read-closed']:x}
ldr r1,[r4,#16]
mov r2,#0
mov r3,#0
ldr ip,=0xb9e9e0
blx ip
pop {{r4-r6,pc}}
'''
    sources['read-tail']=f'''push {{r4-r6,lr}}
ldr r4,=0x{s:x}
str r0,[r4,#20]
add r1,r1,#16
str r1,[r4,#24]
mov r0,#3
str r0,[r4,#8]
dmb sy
ldr r0,=0x{addresses['read-closed']:x}
ldr r1,[r4,#16]
mov r2,#0
mov r3,#0
ldr ip,=0xb9e9e0
blx ip
pop {{r4-r6,pc}}
'''
    sources['read-closed']=f'''push {{r4-r8,lr}}
ldr r4,=0x{s:x}
str r0,[r4,#28]
cmp r0,#0
bne load_error
ldr r0,[r4,#20]
cmp r0,#0
bne loaded
ldr r8,[r4,#24]
ldr r5,=0x{r:x}
ldmia r5,{{r0-r3}}
ldr ip,=0x4341504f
cmp r0,ip
bne loaded
cmp r1,#1
beq v1
cmp r1,#2
bne loaded
cmp r8,#20
bne loaded
ldr r7,[r5,#16]
cmp r7,#4
bhi loaded
b validate
v1:
cmp r8,#16
bne loaded
ldr r7,=0x{b+16:x}
ldr r7,[r7]
validate:
cmp r2,#{p['minimum_menu_percent']}
blo loaded
cmp r2,#100
cmpls r3,#100
bhi loaded
ldr r5,=0x{o:x}
ldr r6,=0x{b:x}
ldr ip,[r5,#8]
ldr r0,[r6,#8]
cmp ip,r0
ldreq ip,[r5,#12]
ldreq r0,[r6,#12]
cmpeq ip,r0
str r2,[r6,#8]
str r3,[r6,#12]
streq r2,[r5,#8]
streq r3,[r5,#12]
ldr r5,=0x{theme:x}
ldr ip,[r5]
ldr r0,[r6,#16]
cmp ip,r0
str r7,[r6,#16]
streq r7,[r5]
loaded:
ldr r0,[r4,#48]
add r0,r0,#1
str r0,[r4,#48]
mov r0,#4
dmb sy
str r0,[r4,#8]
pop {{r4-r8,pc}}
load_error:
mov r0,#0x91
str r0,[r4,#44]
mov r0,#0x80
str r0,[r4,#8]
pop {{r4-r8,pc}}
'''
    sources['write-seeked']=replace_once(sources['write-seeked'],'mov r2,#16','mov r2,#20')
    sources['write-done']=replace_once(sources['write-done'],'cmpeq r1,#16','cmpeq r1,#20')
    sources['write-closed']=replace_once(sources['write-closed'],'stmia r6,{r0-r3}',
        'stmia r6,{r0-r3}\nldr r0,[r5,#16]\nstr r0,[r6,#16]')
    patches=json.loads((source/'preferences-patches.json').read_text())['patches']
    replaced=set(addresses.values())|{r,w,b}
    patches=[q for q in patches if int(q['address'],0) not in replaced]
    blocks=[]
    for name,text in sources.items():
        address=addresses[name]
        code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(text,address)[0])
        assert len(code)<=512,(name,len(code))
        path=out/(name+'.bin');path.write_bytes(code)
        (out/(name+'.s')).write_text(text)
        blocks.append({'name':name,'address':hex(address),'bytes':len(code),'sha256':hashlib.sha256(code).hexdigest()})
        patches.append({'address':hex(address),'file':str(path),'what':'Versioned opacity/theme preferences callback'})
    for name,address in [('read-buffer',r),('write-buffer',w),('baseline',b)]:
        path=out/(name+'.bin');path.write_bytes(bytes(20))
        patches.append({'address':hex(address),'file':str(path),'what':'Bounded twenty-byte preferences buffer'})
    p.update(blocks=blocks,file_schema=2,record_bytes=20,theme_state=hex(theme),
             save_on_confirmed_f1=args.save_confirmed_f1,
             v1_compatible=True,limitations=['Internal v2 callback prototype; full SD/cold-boot qualification pending'])
    (out/'provenance.json').write_text(json.dumps(p,indent=2)+'\n')
    (out/'preferences-patches.json').write_text(json.dumps({'patches':patches},indent=2)+'\n')
    print('PREPARED schema2/20-byte preferences; original schema1 READ16 retained')


if __name__=='__main__':main()
