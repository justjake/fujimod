#!/usr/bin/env python3
"""Build native F1 framing guides, owned GVFF resources and schema3 preferences."""
import os
from pathlib import Path
import hashlib,json,math,struct,sys
ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM
from firmware_image import FirmwareImage
from xpro2_patch_layout import check_module_allocation
BASE,END,PAGE=0xa9000,0xb9000,0xd3a828
VENEER_BASE,VENEER_END=0x620000,0x621000
STATE,DESC,CHOICES,LABELS=BASE,BASE+0x20,BASE+0x40,BASE+0xa0
FCHOICES,FLABELS,TEXT_PTR,TEXT_DATA,HEADERS=BASE+0x300,BASE+0x360,BASE+0x600,BASE+0x680,BASE+0xc00
VALIDATOR,DRAW,DIVIDE=BASE+0x1000,BASE+0x2400,BASE+0x2b00
RESOURCE_APIS=[0x21dd600,0x21dd65c,0x21dd6b8,0x21dd714]
GUARDS=[0x1fb8240,0x1fb9e34,0x1fba158,0x1fba404,0x1fbabdc,0x21d8a28,0x21d8ae0,0x21dd3bc,0x21dd878,0x21dd970]
DRAW_APIS=[0x20a5890,0x20a60ec,0x20a7848,0x20a7fa8,0x20a860c]
CHOICE_KEYS=[2,0,3,1,4,5,6,7,8,9]
NAMES=['NONE','GRID 9','GRID 24','HD FRAMING','GOLDEN RATIO','SPIRAL BOTTOM LEFT',
       'SPIRAL BOTTOM RIGHT','SPIRAL TOP LEFT','SPIRAL TOP RIGHT','DYNAMIC SYMMETRY']

def branch(a,b):
    delta=b-a-8;assert delta%4==0 and -(1<<25)<=delta<(1<<25)
    return 0xea000000|((delta//4)&0xffffff)

def gvff(paths,width=640,height=480):
    """Stock GVFF2.1 filled black paths, using the reference paint/key layout."""
    out=bytearray(b'GVFF'+struct.pack('<9I4f',0,0x10002,0,56,len(paths),1,0,0x29a,0,0,height,width,0))
    out.extend(struct.pack('<2I5f2I',0,0x1b00,0,0,0,1,0,0,0))
    for points in paths:
        segments=bytes([2]+[4]*(len(points)-1)+[0])
        coords=b''.join(struct.pack('<2f',x,height-y) for x,y in points)
        out.extend(struct.pack('<9I2f2I',2,0,1,0,0,0x29a,0x1900,0x1800,0x1700,1,10,len(segments),len(coords)))
        out.extend(segments);out.extend(bytes((-len(segments))%4));out.extend(coords)
    return bytes(out)

def stroke(a,b,width=2):
    dx,dy=b[0]-a[0],b[1]-a[1];length=math.hypot(dx,dy)
    ox,oy=-dy/length*width/2,dx/length*width/2
    return [(a[0]+ox,a[1]+oy),(b[0]+ox,b[1]+oy),(b[0]-ox,b[1]-oy),(a[0]-ox,a[1]-oy)]

def main():
    f=FirmwareImage();stock=(ROOT.parent/'fujiextra2/build/ffboot_stock.bin').read_bytes()
    recipe=next(v for v in json.loads((ROOT.parent/'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json').read_text()) if v['firmware_id']=='xpro2-511')
    placement=check_module_allocation(stock,recipe['plan'],BASE,END)
    veneer_placement=check_module_allocation(stock,recipe['plan'],VENEER_BASE,VENEER_END)
    out=ROOT/'analysis/xpro2-framing-internal';out.mkdir(exist_ok=True)
    source=ROOT/'analysis/xpro2-f1-last-setup-generic-internal/patches.json'
    patches=json.loads(source.read_text())['patches'];module=bytearray(END-BASE);spans=[];blocks=[]
    def put(a,data,name):
        assert BASE<=a and a+len(data)<=END
        assert all(a+len(data)<=x or y<=a for x,y in spans),name
        module[a-BASE:a-BASE+len(data)]=data;spans.append((a,a+len(data)))
    ks=Ks(KS_ARCH_ARM,KS_MODE_ARM)
    def asm(a,text,name):
        code=bytes(ks.asm(text,a)[0]);put(a,code,name);(out/(name+'.s')).write_text(text)
        blocks.append({'name':name,'address':hex(a),'bytes':len(code),'sha256':hashlib.sha256(code).hexdigest()})
        return code
    by_address={int(p['address'],0):p for p in patches};veneers=bytearray()
    def chain(a):
        p=by_address[a];word=int(p['value'],0);assert word>>24==0xea
        imm=word&0xffffff
        if imm&0x800000:imm-=0x1000000
        return a+8+imm*4
    def install(a,target,what):
        nonlocal patches
        if not -(1<<25)<=target-a-8<(1<<25):
            at=VENEER_BASE+len(veneers)
            veneers.extend(struct.pack('<2I',0xe51ff004,target));target=at
        patches=[p for p in patches if int(p['address'],0)!=a]
        patches.append({'address':hex(a),'value':hex(branch(a,target)),'what':what})
    # Preserve the six original connection rows and all three verified mods.
    old=(ROOT/'analysis/xpro2-f1-last-setup-generic-internal/last-setup-data.bin').read_bytes()
    pairs=old[:9*8]+struct.pack('<4I',0xf3,DESC,0x7ffffffe,0)
    put(FCHOICES,pairs,'f1-choices')
    rows=bytearray(old[0x60:0x60+9*48]);extra=bytearray(rows[6*48:7*48]);struct.pack_into('<3I',extra,0,0x7007,0x700e,0x700e);rows.extend(extra)
    put(FLABELS,rows,'f1-labels')
    for a,v in [(PAGE+16,FCHOICES),(PAGE+20,FLABELS)]:
        patches=[p for p in patches if int(p['address'],0)!=a];patches.append({'address':hex(a),'value':hex(v),'what':'F1 includes framing guideline after themes'})
    put(STATE,struct.pack('<8I',2,0,0,0,0,0,0,0),'guide-state')
    put(DESC,struct.pack('<BBH6I',72,0,0x109,8,0,PAGE,CHOICES,LABELS,0x7f04),'guide-selector')
    put(CHOICES,b''.join(struct.pack('<2I',v,0) for v in CHOICE_KEYS)+struct.pack('<2I',0x7ffffffe,0),'guide-choices')
    labels=bytearray()
    for i in range(10):
        row=bytearray(rows[6*48:7*48]);struct.pack_into('<3I',row,0,0x700e,0x700f+i,0x700f+i);labels.extend(row)
    put(LABELS,labels,'guide-labels')
    # Keep the original SCREEN SET-UP guide control synchronized with F1.
    for a,v in [(0xd2f334+16,CHOICES),(0xd2f334+20,LABELS)]:
        patches.append({'address':hex(a),'value':hex(v),'what':'Existing guide control shares the extended guide choices'})
    prov=json.loads((ROOT/'analysis/xpro2-f1-theme-menu-internal/provenance.json').read_text())
    text=prov['text'];names=text['strings']+['FRAMING GUIDELINE']+NAMES
    names[7]='FujiMods'  # Owned folder caption 0x7007; keep feature IDs unchanged.
    assert len(names)==25
    pointers=[];cursor=TEXT_DATA
    for name in names:
        value=name.encode('utf-16-le')+b'\0\0';pointers.append(cursor);put(cursor,value,'caption '+name);cursor+=len(value)
    assert cursor<HEADERS
    put(TEXT_PTR,struct.pack('<25I',*pointers),'caption-pointers')
    text_hook=next(h for h in prov['hooks'] if h['target']=='0x1fbafec')['assembly']
    assert text_hook.count('cmp r2,#13')==1 and text_hook.count('ldr r1,=0x100c00')==1
    text_hook=text_hook.replace('cmp r2,#13','cmp r2,#24').replace('ldr r1,=0x100c00',f'ldr r1,=0x{TEXT_PTR:x}')
    text_code=bytes(ks.asm(text_hook,0x100b00)[0]);old_text=by_address[0x100b00];assert len(text_code)==Path(old_text['file']).stat().st_size
    path=out/'caption-renderer.bin';path.write_bytes(text_code)
    patches=[{**p,'file':str(path)} if int(p['address'],0)==0x100b00 else p for p in patches]
    # Golden ratio matches the shipped 0.381966 split. Spiral GVFF bytes are
    # copied unchanged; Dynamic Symmetry is an independently generated armature.
    split=0.381966;gold=[]
    for x in [round(640*split),640-round(640*split)-2]:gold.append([(x,0),(x+2,0),(x+2,480),(x,480)])
    for y in [round(480*split),480-round(480*split)-2]:gold.append([(0,y),(640,y),(640,y+2),(0,y+2)])
    assets=[('golden-ratio',640,480,gvff(gold),'generated')]
    ref=(ROOT/'analysis/fujimod-release-0.0.1/mod-region-00390000.bin').read_bytes()
    assert hashlib.sha256(ref).hexdigest()=='c7413cecadedb8cf66282d0b403af6f6395c72dae1136ceb31266112867dc200'
    for i,pointer in enumerate(struct.unpack_from('<4I',ref,0x435124-0x390000)):
        magic,dims,payload,size,flags=struct.unpack_from('<5I',ref,pointer-0x390000);assert magic==0x58424d52 and flags==0
        data=ref[payload-0x390000:payload-0x390000+size];assert data[:4]==b'GVFF' and len(data)==size
        assets.append((NAMES[5+i].lower().replace(' ','-'),dims&0xffff,dims>>16,data,hex(pointer)))
    for name,ratio in [('dynamic-3-2',1.5),('dynamic-16-9',16/9),('dynamic-1-1',1)]:
        t=ratio*ratio/(ratio*ratio+1);u=1-t
        lines=[((0,0),(1,1)),((0,1),(1,0)),((1,0),(t,t)),((0,1),(u,u)),((0,0),(u,1-u)),((1,1),(t,1-t))]
        paths=[stroke((a[0]*640,a[1]*480),(b[0]*640,b[1]*480)) for a,b in lines]
        assets.append((name,640,480,gvff(paths),'generated'))
    resource_records=[];cursor=BASE+0x6000
    for i,(name,w,h,data,origin) in enumerate(assets):
        cursor=(cursor+3)&~3;header=HEADERS+i*20
        put(header,struct.pack('<IHH3I',0x58424d52,w,h,cursor,len(data),0),'resource-header '+name)
        put(cursor,data,'gvff '+name);resource_records.append({'name':name,'header':hex(header),'data':hex(cursor),'bytes':len(data),'width':w,'height':h,'sha256':hashlib.sha256(data).hexdigest(),'origin':origin});cursor+=len(data)
    assert len(assets)==8
    asm(VALIDATOR,f'''ldr ip,=0x{HEADERS:x}
mov r1,#8
check:
cmp r0,ip
beq valid
add ip,ip,#20
subs r1,r1,#1
bne check
mov r0,#0
bx lr
valid:
ldr r1,[r0]
ldr ip,=0x58424d52
cmp r1,ip
movne r0,#0
bx lr
''','resource-validator')
    for i,target in enumerate(RESOURCE_APIS):
        a=BASE+0x1100+i*0x100;assert f.words(target,1)[0]==0xe52db004
        result=['ldrh r0,[r4,#4]','ldrh r0,[r4,#6]','mov r0,#1','ldr r0,[r4,#8]'][i]
        asm(a,f'''push {{r1-r4,ip,lr}}
mov r4,r0
bl 0x{VALIDATOR:x}
cmp r0,#0
beq fallback
{result}
pop {{r1-r4,ip,pc}}
fallback:
mov r0,r4
pop {{r1-r4,ip,lr}}
.word 0xe52db004
ldr pc,=0x{target+4:x}
''','resource-api-'+str(i));install(target,a,'Owned GVFF resource metadata; original resources unchanged')
    for i,target in enumerate(GUARDS):
        assert f.words(target,1)[0]==0xe1520003
        a=BASE+0x1500+i*0x80
        asm(a,f'''push {{r0-r3,ip,lr}}
sub r0,r2,#0x1000000
bl 0x{VALIDATOR:x}
cmp r0,#0
pop {{r0-r3,ip,lr}}
bne owned
cmp r2,r3
ldr pc,=0x{target+4:x}
owned:
cmp r0,r0
ldr pc,=0x{target+4:x}
''','resource-guard-'+str(i));install(target,a,'Allow only whitelisted owned GVFF headers through resource bounds check')
    bindings=[('guide-get',0x13d7b40,BASE+0x2000,'r3'),('guide-set',0x13d3d54,BASE+0x2100,'r1'),('guide-available',0x13d95e8,BASE+0x2200,'r0'),('guide-summary',0x1fedd38,BASE+0x2300,'r0')]
    for name,target,a,prop in bindings:
        body=f'movw ip,#0x109\ncmp {prop},ip\nbne fallback\n'
        if name=='guide-get':body+=f'ldr ip,=0x{STATE:x}\nldr r0,[ip]\n'
        if name=='guide-summary':body+=f'ldr ip,=0x{STATE:x}\nldr r0,[ip]\n'
        if name in ('guide-get','guide-summary'):
            body=f'''movw ip,#0x2a1
cmp {prop},ip
beq lcd_visibility
{body}b finish
lcd_visibility:
ldr ip,=0x{STATE:x}
ldr r0,[ip]
cmp r0,#2
moveq r0,#1
movne r0,#0
'''
        if name=='guide-set':body+=f'''cmp r2,#9
movhi r0,#0
bhi finish
cmp r0,#3
ldr ip,=0x{STATE:x}
streq r2,[ip]
dmb sy
mov r0,#1
'''
        if name=='guide-set':
            body=f'''movw ip,#0x2a1
cmp r1,ip
bne guide_value
cmp r2,#1
movhi r0,#0
bhi finish
cmp r0,#3
bne handled
ldr ip,=0x{STATE:x}
cmp r2,#1
beq disable_lcd
ldr r3,[ip]
cmp r3,#2
moveq r3,#0
str r3,[ip]
b handled
disable_lcd:
mov r3,#2
str r3,[ip]
handled:
dmb sy
mov r0,#1
b finish
guide_value:
{body}'''
        if name=='guide-available':
            body=f'''cmp r0,#6
bne guide
cmp r1,#0xf3
bne fallback
ldr ip,=0x1003980
ldr ip,[ip]
ldr r2,=0x{PAGE:x}
cmp ip,r2
bne fallback
mov r0,#0
b finish
guide:
movw ip,#0x109
cmp r0,ip
bne fallback
cmp r1,#9
movls r0,#0
movhi r0,#3
'''
        extra='mov r1,#0\n' if name=='guide-summary' else ''
        asm(a,f'''push {{r1-r3,ip}}
{body}finish:
pop {{r1-r3,ip}}
{extra}bx lr
fallback:
pop {{r1-r3,ip}}
b 0x{chain(target):x}
''',name);install(target,a,'Framing setting109 and F1 row; chain existing property hooks')
    # The original group wrappers retain clipping and their frame scheduling.
    # Replace leaf geometry only for NONE or the added guides; stock grids run
    # their original code and see their original numeric settings0/3/1.
    for i,target in enumerate(DRAW_APIS):
        a=BASE+0x2c00+i*0x100;original=f.words(target,1)[0]
        asm(a,f'''push {{r0-r3,ip,lr}}
ldr ip,=0x{STATE:x}
ldr ip,[ip]
cmp ip,#2
beq done
cmp ip,#4
blo fallback
bl 0x{DRAW:x}
done:
pop {{r0-r3,ip,lr}}
bx lr
fallback:
pop {{r0-r3,ip,lr}}
.word 0x{original:08x}
ldr pc,=0x{target+4:x}
''','guide-leaf-'+str(i));install(target,a,'Native framing geometry adapter; preserve stock grid paths')
    asm(DIVIDE,'''mov r2,#0
mov r3,#0
mov ip,#32
loop:
movs r0,r0,lsl #1
adc r2,r2,r2
cmp r2,r1
subcs r2,r2,r1
adc r3,r3,r3
subs ip,ip,#1
bne loop
mov r0,r3
bx lr
''','unsigned-divide')
    asm(DRAW,f'''push {{r4-r11,lr}}
sub sp,sp,#116
mov r4,r0
mov r5,r1
mov r0,#0
bl 0x2003eb4
cmp r0,#1
bne done
bl 0x2000e0c
mov r6,r0
bl 0x2000e4c
mov r7,r0
bl 0x2000e8c
mov r8,r0
bl 0x2000ecc
mov r9,r0
cmp r8,#1
cmphs r9,#1
blo done
cmp r8,#2048
cmpls r9,#2048
bhi done
ldr r0,=0x{STATE:x}
str r6,[r0,#4]
str r7,[r0,#8]
str r8,[r0,#12]
str r9,[r0,#16]
mov r10,r8
mov r11,r9
ldr r0,[r0]
sub r0,r0,#4
cmp r0,#5
beq dynamic
str r0,[sp,#88]
cmp r0,#0
beq resource
mov r0,r8
str r0,[sp,#92]
ldr r1,=1000000
mul r2,r0,r1
ldr r0,=1618034
mul r3,r9,r0
cmp r2,r3
blo narrow
mov r0,r3
ldr r2,=500000
add r0,r0,r2
bl 0x{DIVIDE:x}
mov r10,r0
b center
narrow:
ldr r0,[sp,#92]
ldr r1,=1000000
mul r0,r1,r0
ldr r2,=809017
add r0,r0,r2
ldr r1,=1618034
bl 0x{DIVIDE:x}
mov r11,r0
center:
sub r0,r8,r10
add r6,r6,r0,lsr #1
sub r0,r9,r11
add r7,r7,r0,lsr #1
b resource
dynamic:
lsl r0,r8,#2
mov r1,#5
mul r1,r9,r1
cmp r0,r1
movlo r0,#7
blo selected
mov r0,#36
mul r0,r8,r0
mov r1,#59
mul r1,r9,r1
cmp r0,r1
movlo r0,#5
movhs r0,#6
selected:
str r0,[sp,#88]
resource:
ldr r0,=0x{HEADERS:x}
ldr r1,[sp,#88]
add r1,r1,r1,lsl #2
add r0,r0,r1,lsl #2
str r0,[sp,#84]
ldr r1,=0x22cc3b8
mov r2,sp
mov r3,#21
copy:
ldr ip,[r1],#4
str ip,[r2],#4
subs r3,r3,#1
bne copy
ldr r0,[sp,#84]
str r0,[sp,#4]
mov r0,#7
str r0,[sp,#12]
orr r0,r6,r7,lsl #16
str r0,[sp,#16]
orr r0,r4,r5,lsl #16
str r0,[sp,#20]
orr r0,r10,r11,lsl #16
str r0,[sp,#24]
ldr r0,=0xffffff64
str r0,[sp,#60]
mov r0,sp
bl 0x1fbafec
ldr r0,=0x{STATE:x}
ldr r1,[r0,#20]
add r1,r1,#1
str r1,[r0,#20]
ldr r1,[sp,#84]
str r1,[r0,#24]
done:
add sp,sp,#116
pop {{r4-r11,pc}}
''','guide-draw')
    # Extend the existing asynchronous file protocol. Version1 and2 remain
    # readable. Only a confirmed F1 page or a closed menu can schedule writes.
    pref=ROOT/'analysis/xpro2-theme-preferences-v3-internal';changes={}
    for name in ['service','read-done','read-closed','write-seeked','write-done','write-closed']:
        changes[name]=(pref/(name+'.s')).read_text()
    changes['service']=changes['service'].replace('str r0,[r5,#16]',f'str r0,[r5,#16]\nldr r0,=0x{STATE:x}\nldr r0,[r0]\nstr r0,[r5,#20]')
    ready=(ROOT/'analysis/xpro2-f1-last-setup-generic-internal/service-ready-native-f1.s').read_text()
    ready=ready.replace('ldr r5,=0x102580',f'ldr r4,=0x{STATE:x}\nldr r4,[r4]\ncmp r4,#9\nbhi invalid\nldr r5,=0x102580')
    ready=ready.replace('cmpeq ip,r7','cmpeq ip,r7\nldreq ip,[r5,#20]\ncmpeq ip,r4')
    ready=ready.replace('mov r0,#2\nstr r0,[r5,#4]','mov r0,#3\nstr r0,[r5,#4]')
    ready=ready.replace('str r7,[r5,#16]','str r7,[r5,#16]\nstr r4,[r5,#20]');changes['service-ready']=ready
    changes['read-done']=changes['read-done'].replace('cmpeq r2,#2\nbne close','cmp r2,#2\nbeq tail4\ncmp r2,#3\nbne close\nmov r2,#8\nb read_tail\ntail4:\nmov r2,#4\nread_tail:')
    changes['read-done']=changes['read-done'].replace('mov r2,#4\nldr r3,=0x102510','ldr r3,=0x102510')
    changes['read-done']=changes['read-done'].replace('cmp r2,r3\nldreq r2,[r5,#4]','cmp r2,r3\nbne close\nldr r2,[r5,#4]')
    # Derive the closed callback from the already verified reader, changing
    # only version dispatch and adding an independently guarded guide word.
    closed=changes['read-closed']
    closed=closed.replace('cmp r1,#2\nbne loaded\ncmp r8,#20\nbne loaded','cmp r1,#2\nbeq v2\ncmp r1,#3\nbne loaded\ncmp r8,#24\nbne loaded\nldr r8,[r5,#20]\ncmp r8,#9\nbhi loaded\nb theme\nv2:\ncmp r8,#20\nbne loaded\nldr r8,=0x102594\nldr r8,[r8]\ntheme:')
    closed=closed.replace('v1:\ncmp r8,#16\nbne loaded','v1:\ncmp r8,#16\nbne loaded\nldr r8,=0x102594\nldr r8,[r8]')
    closed=closed.replace('streq r7,[r5]\nloaded:',f'streq r7,[r5]\nldr r5,=0x{STATE:x}\nldr ip,[r5]\nldr r0,[r6,#20]\ncmp ip,r0\nstr r8,[r6,#20]\nstreq r8,[r5]\nloaded:')
    changes['read-closed']=closed
    changes['write-seeked']=changes['write-seeked'].replace('mov r2,#20','mov r2,#24')
    changes['write-done']=changes['write-done'].replace('cmpeq r1,#20','cmpeq r1,#24')
    changes['write-closed']=changes['write-closed'].replace('str r0,[r6,#16]','str r0,[r6,#16]\nldr r0,[r5,#20]\nstr r0,[r6,#20]')
    original_blocks={v['name']:int(v['address'],0) for v in json.loads((pref/'provenance.json').read_text())['blocks']}
    for name,assembly in changes.items():
        a=original_blocks[name];code=bytes(ks.asm(assembly,a)[0]);assert len(code)<=512,(name,len(code))
        path=out/('preferences-'+name+'.bin');path.write_bytes(code);(out/('preferences-'+name+'.s')).write_text(assembly)
        patches=[p for p in patches if int(p['address'],0)!=a];patches.append({'address':hex(a),'file':str(path),'what':'Guide-aware schema3 preferences with legacy readers'})
    for a in [0x102500,0x102540,0x102580]:
        p=next(p for p in patches if int(p['address'],0)==a);data=Path(p['file']).read_bytes();assert len(data)==20
        path=out/f'preferences-buffer-{a:x}.bin';path.write_bytes(data+bytes(4));p['file']=str(path)
    path=out/'framing-module.bin';path.write_bytes(module)
    patches.append({'address':hex(BASE),'file':str(path),'what':'Owned framing guide module, captions and bounded GVFF resources'})
    assert len(veneers)<=VENEER_END-VENEER_BASE
    path=out/'far-veneers.bin';path.write_bytes(veneers)
    patches.append({'address':hex(VENEER_BASE),'file':str(path),'what':'Absolute veneers for native SDK branches beyond ARM B range'})
    ranges=[]
    for p in patches:
        a=int(p['address'],0);n=Path(p['file']).stat().st_size if 'file'in p else 4
        assert all(a+n<=x or y<=a for x,y in ranges),'Overlapping patches'
        ranges.append((a,a+n))
    (out/'patches.json').write_text(json.dumps({'patches':patches},indent=2)+'\n')
    (out/'provenance.json').write_text(json.dumps({'schema':1,'source_patches_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),
        'placement':placement,'veneer_placement':veneer_placement,'state':hex(STATE),'selector':hex(DESC),'setting':'0x109','page':hex(PAGE),
        'choice_keys':CHOICE_KEYS,'choice_names':NAMES,'blocks':blocks,'resources':resource_records,
        'resource_apis':list(map(hex,RESOURCE_APIS)),'resource_guards':list(map(hex,GUARDS)),
        'draw_apis':list(map(hex,DRAW_APIS)),'preferences_schema':3,'preferences_bytes':24,
        'limitations':['Candidate requires ARM, GPU drawing, UI and SD cold-cycle qualification',
                       'Dynamic Symmetry is a geometric reconstruction, not extracted upstream source',
                       'Physical EVF/OVF and camera behavior not qualified']},indent=2)+'\n')
    print('PREPARED FRAMING GUIDELINE:',len(patches),'patches;',len(assets),'GVFF resources')

if __name__=='__main__':main()
