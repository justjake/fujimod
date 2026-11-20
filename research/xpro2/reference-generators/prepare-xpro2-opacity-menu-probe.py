#!/usr/bin/env python3
"""Build an INTERNAL native-menu integration probe, not the completed opacity UI.

Appends two properties to the stock IQ menu and routes their original getter,
availability and confirmation calls to native opacity state. Captions are
borrowed diagnostic placeholders pending custom text integration. Never stage
this probe as the owner-facing feature or an installable camera update.
"""
import os
from pathlib import Path
import argparse,hashlib,json,struct,sys
from firmware_image import FirmwareImage
from xpro2_patch_layout import check_module_allocation
ROOT=Path(os.environ['XPRO2_LAB_ROOT'])
BASE=0x100000
END=0x101000
MENU=0xd374e8
STATE=0x103b00
PROPERTIES=(0x45a,0x45b)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--text',action='store_true',help='Use owned UTF16 captions through the original native drawing API')
    parser.add_argument('--out',type=Path,help='Separate internal generated output directory')
    parser.add_argument('--f1',action='store_true',help='Group added controls inside one F1 menu entry')
    parser.add_argument('--theme',action='store_true',help='Add the internal five-theme selector under F1')
    args=parser.parse_args()
    if args.f1 and not args.text:raise ValueError('F1 requires owned native text')
    if args.theme and not args.f1:raise ValueError('Theme selector requires F1')
    from keystone import Ks,KS_ARCH_ARM,KS_MODE_ARM
    f=FirmwareImage();stock=(ROOT.parent/'fujiextra2/build/ffboot_stock.bin').read_bytes()
    assert hashlib.sha256(stock).hexdigest()=='ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172'
    assert stock[BASE:END]==bytes(END-BASE)
    recipe_path=ROOT.parent/'fujiextra2/data/ref/fujimod-upstream/catalog/recovery/recipes.generated.json'
    recipe=next(x for x in json.loads(recipe_path.read_text()) if x['firmware_id']=='xpro2-511')
    plan=recipe['plan']
    placement=check_module_allocation(stock,plan,BASE,END)
    fields=f.words(MENU,7);items=f.read(fields[4],19*8)
    f1_group=143
    if args.f1:
        catalog_ids=[]
        for at in range(0xd30920,0xd30920+0x1000,8):
            ident,pointer=f.words(at,2)
            if pointer==0:break
            catalog_ids.append(ident&255)
        assert f1_group not in catalog_ids,'F1 group must not replace an original menu'
    assert struct.unpack_from('<2I',items,18*8)==(0x7ffffffe,0)
    metadata=f.read(fields[5],18*48)
    payload=bytearray(END-BASE);spans=[]
    def put(address,data):
        assert BASE<=address and address+len(data)<=END
        payload[address-BASE:address-BASE+len(data)]=data
        spans.append((address,address+len(data)))
    # Layout kept explicit for provenance and future allocation checks.
    entries=BASE;labels=BASE+0x100;children=BASE+0x500;choices=BASE+0x540
    choice_labels=BASE+0x600
    category,category_choices,category_labels=BASE+0x700,BASE+0x720,BASE+0x740
    theme_descriptor,theme_choices,theme_labels,theme_state=BASE+0x7d0,0x90800,0x90840,0x90700
    theme_data=None
    put(entries,items[:-8]+(struct.pack('<4I',0xf0,category,0x7ffffffe,0) if args.f1 else struct.pack('<6I',0xf0,children,0xf1,children+28,0x7ffffffe,0)))
    root_labels=bytearray(metadata+metadata[:48]*2)
    token_base=0x7000
    if args.text:
        for i in range(2):struct.pack_into('<I',root_labels,(18+i)*48+4,token_base+i)
    if args.f1:
        root_labels=root_labels[:19*48]
        struct.pack_into('<I',root_labels,18*48+4,token_base+7)
    put(labels,root_labels)
    put(choices,b''.join(struct.pack('<2I',i,0) for i in range(5))+struct.pack('<2I',0x7ffffffe,0))
    # Diagnostic labels only. They deliberately do not claim final captions.
    source_child=f.words(struct.unpack_from('<I',items,4)[0],7)
    source_choices=bytearray(f.read(source_child[5],5*48))
    if args.text:
        for i in range(5):
            for field in (1,2):struct.pack_into('<I',source_choices,i*48+field*4,token_base+2+i)
    put(choice_labels,source_choices)
    # Menu25..100%, indicators0..100%, matching the public feature ranges.
    menu_choices,menu_labels=BASE+0x580,BASE+0xf40
    if args.f1:
        put(menu_choices,b''.join(struct.pack('<2I',i,0) for i in range(1,5))+struct.pack('<2I',0x7ffffffe,0))
        put(menu_labels,source_choices[48:])
    for i,prop in enumerate(PROPERTIES):
        put(children+28*i,struct.pack('<BBH6I',f1_group if args.f1 else 44,0,prop,8,0,category if args.f1 else MENU,
            menu_choices if args.f1 and i==0 else choices,menu_labels if args.f1 and i==0 else choice_labels,0x7f00+i))
    if args.f1:
        # Native nested categories use type4/property6 (e.g. stock d67e4c).
        put(category,struct.pack('<BBH6I',f1_group,4,6,8,0,MENU,category_choices,category_labels,0x7f02))
        pairs=[0,children,1,children+28]+([2,theme_descriptor] if args.theme else [])+[0x7ffffffe,0]
        put(category_choices,struct.pack('<'+str(len(pairs))+'I',*pairs))
        rows=bytearray(metadata[:48]*(3 if args.theme else 2))
        for i in range(3 if args.theme else 2):
            caption=token_base+(8 if i==2 else i)
            struct.pack_into('<3I',rows,i*48,token_base+7,caption,caption)
        put(category_labels,rows)
        if args.theme:
            put(theme_descriptor,struct.pack('<BBH6I',f1_group,0,0x45c,8,0,category,theme_choices,theme_labels,0x7f03))
            theme_data=bytearray(theme_labels+5*48-theme_choices)
            struct.pack_into('<12I',theme_data,0,*[v for i in range(5) for v in (i,0)],0x7ffffffe,0)
            for i in range(5):
                at=theme_labels-theme_choices+i*48
                theme_data[at:at+48]=source_choices[:48]
                struct.pack_into('<3I',theme_data,at,token_base+8,token_base+9+i,token_base+9+i)
    hooks=[]
    def hook(target,address,body,allow_fp_spill=False):
        original=f.read(target,4)
        assert original==stock[target:target+4]
        ins=f.disasm(target,4)[0]
        assert ins.mnemonic=='push' or (allow_fp_spill and struct.unpack('<I',original)[0]==0xe52db004),ins.mnemonic
        # Internal stock call sites may retain caller-saved values based on
        # the known callee implementation. Preserve them on every owned path
        # and restore the exact entry state before replaying the prologue.
        body=body.replace('bxhi lr','pophi {r1-r3,ip}\nbxhi lr')
        body=body.replace('bx lr','pop {r1-r3,ip}\nbx lr')
        source='push {r1-r3,ip}\n'+body+f'\nfallback:\npop {{r1-r3,ip}}\n.word 0x{struct.unpack("<I",original)[0]:08x}\nb 0x{target+4:x}\n'
        code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(source,address)[0])
        assert len(code)<=0x100
        put(address,code)
        delta=address-target-8;assert delta%4==0 and -(1<<25)<=delta<(1<<25)
        word=0xea000000|((delta//4)&0xffffff)
        hooks.append({'target':hex(target),'address':hex(address),'bytes':len(code),
                      'expected_original_word':hex(struct.unpack('<I',original)[0]),'branch_word':hex(word),
                      'assembly':source,'sha256':hashlib.sha256(code).hexdigest()})
    if args.f1:
        hook(0xab9e90,BASE+0xf00,f'''cmp r0,#{f1_group}
bne fallback
ldr r0,=0x{category:x}
bx lr''',allow_fp_spill=True)
        # Keep the F1 entry's parent value blank, while its own page uses
        # metadata field1 for feature names and the ordinary value captions.
        if not args.theme:
            hook(0x126a7c4,BASE+0x7a0,'''cmp r0,#44
bne fallback
movw ip,#0x45c
cmp r1,ip
bne fallback
cmp r3,#1
bne fallback
mov r0,#0
bx lr''')
            assert hooks[-1]['bytes']<=0x60
    category_get='''movw ip,#0x45c
cmp r3,ip
bne regular_get
mov r0,#0
bx lr
regular_get:
''' if args.f1 else ''
    if args.theme:
        category_get=f'''movw ip,#0x45c
cmp r3,ip
bne regular_get
ldr ip,=0x{theme_state:x}
ldr r0,[ip]
cmp r0,#4
movhi r0,#0
bx lr
regular_get:
'''
    hook(0x13d7b40,BASE+0x800,category_get+f'''movw ip,#0x45a
cmp r3,ip
beq menu
add ip,ip,#1
cmp r3,ip
bne fallback
ldr ip,=0x{STATE+12:x}
b get
menu:
ldr ip,=0x{STATE+8:x}
get:
ldr r0,[ip]
cmp r0,#100
movhi r0,#100
mov r1,#0
divide:
cmp r0,#25
subhs r0,r0,#25
addhs r1,r1,#1
bhs divide
mov r0,r1
bx lr''')
    category_set='''movw ip,#0x45c
cmp r1,ip
bne regular_set
cmp r2,#1
movls r0,#1
movhi r0,#0
bx lr
regular_set:
''' if args.f1 else ''
    if args.theme:
        category_set=f'''movw ip,#0x45c
cmp r1,ip
bne regular_set
cmp r2,#4
movhi r0,#0
bxhi lr
cmp r0,#3
ldr ip,=0x{theme_state:x}
streq r2,[ip]
dmb sy
mov r0,#1
bx lr
regular_set:
'''
    minimum_check='''movw r3,#0x45a
cmp r1,r3
bne valid_set
cmp r2,#0
bne valid_set
mov r0,#0
bx lr
valid_set:
''' if args.f1 else ''
    hook(0x13d3d54,BASE+0x900,category_set+f'''movw ip,#0x45a
cmp r1,ip
beq menu
add ip,ip,#1
cmp r1,ip
bne fallback
ldr ip,=0x{STATE+12:x}
b set
menu:
ldr ip,=0x{STATE+8:x}
set:
cmp r2,#4
movhi r0,#0
bxhi lr
{minimum_check}\
cmp r0,#3
bne owned
mov r3,#25
mul r2,r3,r2
str r2,[ip]
dmb sy
owned:
mov r0,#1
bx lr''')
    category_available='''movw ip,#0x45c
cmp r0,ip
bne regular_available
cmp r1,#1
movls r0,#0
movhi r0,#3
bx lr
regular_available:
''' if args.f1 else ''
    if args.theme:category_available=category_available.replace('cmp r1,#1','cmp r1,#4')
    minimum_available='''movw ip,#0x45a
cmp r0,ip
bne valid_option
cmp r1,#0
bne valid_option
mov r0,#3
bx lr
valid_option:
''' if args.f1 else ''
    hook(0x13d95e8,BASE+0xa00,category_available+minimum_available+'''movw ip,#0x45a
cmp r0,ip
beq custom
add ip,ip,#1
cmp r0,ip
beq custom
cmp r0,#6
bne fallback
cmp r1,#0xf0
cmpne r1,#0xf1
bne fallback
mov r0,#0
bx lr
custom:
cmp r1,#4
movls r0,#0
movhi r0,#3
bx lr''')
    # The parent-row caption reads through a different stock getter than the
    # option picker. Its return value spans r0:r1; unknown mod properties used
    # to reach the stock configuration store and therefore displayed index0.
    theme_summary=f'''movw ip,#0x45c
cmp r0,ip
bne opacity_summary
ldr ip,=0x{theme_state:x}
ldr r0,[ip]
cmp r0,#4
movhi r0,#0
mov r1,#0
str r1,[sp]
bx lr
opacity_summary:
''' if args.theme else ''
    hook(0x1fedd38,BASE+0xe00,theme_summary+f'''movw ip,#0x45a
cmp r0,ip
beq summary_menu
add ip,ip,#1
cmp r0,ip
bne fallback
ldr ip,=0x{STATE+12:x}
b summary_get
summary_menu:
ldr ip,=0x{STATE+8:x}
summary_get:
ldr r0,[ip]
cmp r0,#100
movhi r0,#100
mov r1,#0
summary_divide:
cmp r0,#25
subhs r0,r0,#25
addhs r1,r1,#1
bhs summary_divide
mov r0,r1
mov r1,#0
str r1,[sp]
bx lr''')
    text_provenance=None
    if args.text:
        names=['MENU OPACITY','INDICATOR OPACITY','0%','25%','50%','75%','100%']+(['F1'] if args.f1 else [])
        if args.theme:names+=['MENU THEME','DEFAULT','X100VI','CUPERTINO','TEENAGE','EVA-01']
        addresses=[];cursor=BASE+0xd00
        for name in names:
            addresses.append(cursor);value=name.encode('utf-16-le')+b'\0\0'
            put(cursor,value);cursor+=len(value)
        if cursor%4:put(cursor,bytes(4-cursor%4))
        assert cursor<=BASE+0xe00
        put(BASE+0xc00,struct.pack('<'+str(len(addresses))+'I',*addresses))
        # Native IQ screens call the generic renderer directly. The resource
        # convenience wrapper 1fba7f4 is not on their observed drawing path.
        target=0x1fbafec;address=BASE+0xb00;original=f.read(target,4)
        assert original==stock[target:target+4] and struct.unpack('<I',original)[0]==0xe92d4ff0
        trampoline=BASE+0xbf0
        put(trampoline,bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm('.word 0xe92d4ff0; b 0x1fbaff0',trampoline)[0]))
        source=f'''push {{r1-r3,ip}}
ldr r1,[r0,#4]
movw r2,#0x{token_base:x}
sub r2,r1,r2
cmp r2,#{len(names)-1}
bhi fallback
push {{r4-r6,lr}}
ldr r1,=0x{BASE+0xc00:x}
ldr r5,[r1,r2,lsl #2]
sub sp,sp,#104
mov r1,r0
mov r2,sp
mov r3,#26
copy:
ldr ip,[r1],#4
str ip,[r2],#4
subs r3,r3,#1
bne copy
mov ip,#0
str ip,[sp,#4]
str r5,[sp,#8]
mov ip,#14
str ip,[sp,#12]
mov ip,#100
strb ip,[sp,#60]
mov ip,#255
strb ip,[sp,#61]
strb ip,[sp,#62]
strb ip,[sp,#63]
mov r0,sp
ldr ip,=0x{trampoline:x}
blx ip
add sp,sp,#104
pop {{r4-r6,lr}}
pop {{r1-r3,ip}}
bx lr
fallback:
pop {{r1-r3,ip}}
.word 0xe92d4ff0
b 0x1fbaff0
'''
        code=bytes(Ks(KS_ARCH_ARM,KS_MODE_ARM).asm(source,address)[0]);assert len(code)<=0xf0
        put(address,code)
        delta=address-target-8;assert delta%4==0 and -(1<<25)<=delta<(1<<25)
        hooks.append({'target':hex(target),'address':hex(address),'bytes':len(code),
                      'expected_original_word':hex(struct.unpack('<I',original)[0]),
                      'branch_word':hex(0xea000000|((delta//4)&0xffffff)),
                      'assembly':source,'sha256':hashlib.sha256(code).hexdigest()})
        text_provenance={'tokens':[hex(token_base+i) for i in range(len(names))],'strings':names,
                         'utf16_addresses':[hex(a) for a in addresses],
                         'renderer':hex(target),'raw_utf16_renderer':'0x1fbafec','native_trampoline':hex(trampoline),
                         'struct_bytes':104,'owned_color':'white, 100% source opacity',
                         'limitations':['Native palette parity and drawing still require full-machine verification']}
    out=args.out.resolve() if args.out else ROOT/('analysis/xpro2-opacity-native-menu-text-internal' if args.text else 'analysis/xpro2-opacity-native-menu-internal')
    if not out.is_relative_to(ROOT/'analysis'):raise ValueError('Internal output must remain inside analysis')
    out.mkdir(exist_ok=True)
    binary=out/'menu-probe.bin';binary.write_bytes(payload)
    previous=json.loads((ROOT/'analysis/xpro2-opacity-menu50-hud100-prototype/opacity-patches.json').read_text())
    # Copy only initialized spans: carrying all the empty alignment padding
    # wastes the fixed, bounded reset-verifier instruction budget.
    merged=[]
    for a,b in sorted(spans):
        if merged and a<=merged[-1][1]:merged[-1]=(merged[-1][0],max(b,merged[-1][1]))
        else:merged.append((a,b))
    parts=[]
    for a,b in merged:
        path=out/f'menu-part-{a:08x}.bin';path.write_bytes(payload[a-BASE:b-BASE])
        parts.append({'address':hex(a),'file':str(path),'what':'INTERNAL native menu integration; no persistence yet'})
    edits=[*previous['patches'],*parts,
           {'address':hex(MENU+16),'value':hex(entries),'what':'Extend stock IQ choices, preserving all original rows'},
           {'address':hex(MENU+20),'value':hex(labels),'what':'Extended stock metadata for the two opacity rows'}]
    edits.extend({'address':h['target'],'value':h['branch_word'],'what':'Native opacity property hook; preserve original prologue on fallback'} for h in hooks)
    if args.theme:
        auxiliary=out/'theme-menu-data.bin';auxiliary.write_bytes(theme_data)
        edits.append({'address':hex(theme_choices),'file':str(auxiliary),'what':'Theme selector data; merge into owned RGB core before building'})
    (out/'menu-patches.json').write_text(json.dumps({'name':'INTERNAL opacity menu integration probe',
        'commento':'Not owner UI yet: native drawing/confirmation and preference persistence require validation. Emulator only.',
        'patches':edits},indent=2)+'\n')
    result={'schema':1,'stock_sha256':hashlib.sha256(stock).hexdigest(),
            'payload':{'address':hex(BASE),'bytes':len(payload),'sha256':hashlib.sha256(payload).hexdigest()},
            'placement':placement,
            'initialized_spans':[[hex(a),hex(b)] for a,b in merged],
            'copied_bytes':sum(b-a for a,b in merged),
            'recipe_sha256':hashlib.sha256(recipe_path.read_bytes()).hexdigest(),
            'new_property_ids':[hex(p) for p in PROPERTIES],'menu':hex(MENU),'rows_before':18,'rows_after':19 if args.f1 else 20,
            'f1':{'descriptor':hex(category),'menu_id':f1_group,'property':'0x6','type':4,'choices':hex(category_choices),'labels':hex(category_labels),'children':3 if args.theme else 2,'menu_minimum_percent':25} if args.f1 else None,
            'theme':{'property':'0x45c','descriptor':hex(theme_descriptor),'choices':hex(theme_choices),'labels':hex(theme_labels),'state':hex(theme_state)} if args.theme else None,
            'children':[hex(children),hex(children+28)],'choices':hex(choices),'hooks':hooks,'text':text_provenance,
            'limitations':['Internal probe, not final UI',*(['Diagnostic borrowed captions'] if not args.text else []),
                           'Confirmation scheduling and native drawing require full-machine validation',
                           'No persistence','Declared-free placement is not hardware qualification']}
    (out/'provenance.json').write_text(json.dumps(result,indent=2)+'\n')
    print('PREPARED internal native menu probe:',len(edits),'declared patches')

if __name__=='__main__':main()
