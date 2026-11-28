#!/usr/bin/env python3
"""Reproduce the X-Pro2 laboratory from user-supplied, hash-bound public inputs.

No camera connection, firmware installation, input download or source checkout
is performed. The sparse recipe is data, never executed as Python or assembly.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import zipfile

HERE=Path(__file__).resolve().parent
DAT_SHA='6f9792e53a776ee116732a6df4425b0f5393ad2f53655f573924afe9c61dc54d'
PATCH_SHA='ffdf9108f9ec49bd495d01990edbc91ed161f60549feb8ee08c4e483f1c89486'
ROM_SHA='632445259ac679bf36281ea3f707b127759e0802bd507ba2b8d5b926492fd59f'
MOD_SHA='c7413cecadedb8cf66282d0b403af6f6395c72dae1136ceb31266112867dc200'
SD_SHA='09ef2a0f2719635d1b7597424035bcaae6c605e39b163946311e4854ee45ef56'
INVERT=bytes(reversed(range(256)))


def digest(data):return hashlib.sha256(data).hexdigest()
def require(value,message):
    if not value:raise ValueError(message)
def write_json(path,value):path.write_text(json.dumps(value,indent=2)+'\n')


def method4(raw,offset):
    require(offset+20<=len(raw),'truncated method-4 header')
    unpacked,packed,pages,page_size,method=struct.unpack_from('<5I',raw,offset)
    require(page_size==0x4000 and method==4 and packed>=20 and offset+packed<=len(raw),'invalid stream geometry')
    require(pages==(packed+page_size-1)//page_size,'invalid stream page count')
    require(unpacked<=64*1024*1024,'decoded stream exceeds bound')
    data=raw[offset+20:offset+packed];result=bytearray();cursor=0
    while cursor<len(data):
        first=data[cursor];cursor+=1
        if first<0x80:
            count=first
            require(count>0 and cursor+count<=len(data),'invalid literal')
            result.extend(data[cursor:cursor+count]);cursor+=count
        else:
            require(cursor<len(data),'truncated match')
            second=data[cursor];cursor+=1
            distance=((first&0x7f)<<4)|(second>>4);count=second&15
            require(0<distance<=len(result) and count>0,'invalid match')
            for _ in range(count):result.append(result[-distance])
        require(len(result)<=unpacked,'decoded stream overflow')
    require(len(result)==unpacked,'decoded stream size mismatch')
    return bytes(result)


def stock_image(dat,plan):
    require(digest(dat)==DAT_SHA,'X-Pro2 5.11 DAT hash mismatch')
    decoded=dat.translate(INVERT);image=bytearray(plan['size'])
    for region in plan['regions']:
        source=region['source'];offset=source['dat_offset']
        value=method4(decoded,offset) if source['compression']=='method4' else decoded[offset:offset+source['size']]
        require(len(value)==source['decoded_size'] and digest(value)==source['sha256'],'region identity mismatch: '+region['name'])
        a=region['address'];require(0<=a<=a+len(value)<=len(image),'region outside resident image')
        image[a:a+len(value)]=value
    require(digest(image)==plan['stock_sha256'],'stock resident image hash mismatch')
    return image


def mod_reference(patch):
    require(digest(patch)==PATCH_SHA,'public X70 reference patch hash mismatch')
    require(patch[:8]==b'FJMODP01' and len(patch)>=12,'invalid patch container')
    size=struct.unpack_from('<I',patch,8)[0]
    require(0<size<=1048576 and 12+size<=len(patch),'invalid patch metadata length')
    meta=json.loads(patch[12:12+size]);program=patch[12+size:]
    require(digest(program)==meta['program_sha256'],'patch program hash mismatch')
    names=[name for name in ['ffboot','ffsys','fpupdate'] if name in meta['outputs']]
    base,end=0x390000,0x55b8c0;result=bytearray(end-base);cursor=0
    for _ in range(meta['commands']):
        require(cursor+20<=len(program),'truncated reference command')
        op,target,flags,reserved,dest,length,source,packed=struct.unpack_from('<4B4I',program,cursor);cursor+=20
        require(target<len(names) and reserved==0 and flags in (0,1) and length>0,'invalid reference command')
        require(dest+length<=meta['outputs'][names[target]]['bytes'],'reference command outside output')
        overlaps=names[target]=='ffboot' and dest<end and dest+length>base
        if op==3:
            require(flags==0 and source==0 and packed==0 and cursor+length<=len(program),'invalid literal command')
            data=program[cursor:cursor+length];cursor+=length
            if overlaps:
                left=max(dest,base);right=min(dest+length,end)
                result[left-base:right-base]=data[left-dest:right-dest]
        else:
            require(op in (1,2) and not overlaps,'non-literal command intersects reference module')
    require(cursor==len(program) and digest(result)==MOD_SHA,'reference module identity mismatch')
    return bytes(result)


def apply_delta(stock,reference,text):
    result=bytearray(stock);cursor=None;previous_end=0
    for line in text.splitlines():
        line=line.strip()
        if not line or line.startswith('#'):continue
        match=re.fullmatch(r'\.org (0x[0-9a-f]+)',line)
        if match:
            cursor=int(match[1],16);require(cursor>=previous_end,'overlapping or unordered delta');continue
        require(cursor is not None,'delta data before offset')
        match=re.fullmatch(r'\.incbin "mod-reference", (0x[0-9a-f]+), (0x[0-9a-f]+)',line)
        if match:
            start,size=map(lambda value:int(value,16),match.groups())
            require(0<=start<start+size<=len(reference),'reference slice outside input')
            data=reference[start:start+size]
        else:
            require(line.startswith('.byte '),'unsupported delta directive')
            values=line[6:].split(',')
            require(1<=len(values)<=16 and all(re.fullmatch(r'0x[0-9a-f]{2}',v) for v in values),'invalid byte directive')
            data=bytes(int(v,16) for v in values)
        require(cursor+len(data)<=len(result),'delta outside resident image')
        result[cursor:cursor+len(data)]=data;cursor+=len(data);previous_end=cursor
    require(digest(result)==SD_SHA,'modified SD image hash mismatch')
    return bytes(result)


def read_rom(path):
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            info=archive.getinfo('xt2-441-bootrom.bin')
            require(info.file_size==65536,'unexpected public ROM attachment size')
            raw=archive.read(info)
        require(digest(raw)=='38682a355f67eccd98989e812ff094e88c44cde1172c3d351c7f29c4c15d0876','public ROM attachment hash mismatch')
        raw=raw[:32768]
    else:
        with path.open('rb') as handle:raw=handle.read(32769)
    require(len(raw)==32768 and digest(raw)==ROM_SHA,'32 KiB X-T2 reference ROM hash mismatch')
    return raw


def prepare(dat_path,patch_path,rom_path,out):
    require(not out.exists(),'output directory already exists')
    dat=dat_path.read_bytes();plan=json.loads((HERE/'data/resident-plan.json').read_text())
    stock=stock_image(dat,plan)
    reference=mod_reference(patch_path.read_bytes())
    sd=apply_delta(stock,reference,(HERE/'legacy/firmware-delta.generated.S').read_text())
    rom=read_rom(rom_path);nor=dat[604:].translate(INVERT)
    image=dat+sd+rom+nor
    out.mkdir(parents=True)
    (out/'firmware.input').write_bytes(image)
    (out/'stock-resident.bin').write_bytes(stock)
    (out/'ffboot.bin').write_bytes(sd)
    (out/'ffsys.bin').write_bytes(dat[1248:3296].translate(INVERT))
    for mode in ['A','B']:
        wrapper=json.loads((HERE/f'profiles/rom-{mode}.template.json').read_text())
        wrapper['graph']['firmware'].update(size=len(image),sha256=digest(image))
        write_json(out/f'graph-{mode}.json',wrapper)
    resident=json.loads((HERE/'profiles/resident-B.template.json').read_text())
    resident['graph']['firmware'].update(size=len(image),sha256=digest(image))
    write_json(out/'graph-resident-B.json',resident)
    report={'schema':1,'mode':'offline reference-ROM laboratory',
            'dat_sha256':digest(dat),'stock_sha256':digest(stock),'sd_sha256':digest(sd),
            'rom_sha256':digest(rom),'input_sha256':digest(image),
            'physical_hardware_qualified':False,'installable_dat_produced':False,
            'limitations':['X-T2 reference ROM; X-Pro2 ROM and boot contacts unqualified',
                           'Timing, DDR training and lens/sensor responses are explicit model assumptions',
                           'No AF speed, power or physical recovery measurement']}
    write_json(out/'inputs.json',report)
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dat',type=Path,required=True)
    parser.add_argument('--reference-patch',type=Path,required=True)
    parser.add_argument('--rom',type=Path,required=True,help='Public ROM zip attachment or reviewed 32 KiB prefix')
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    try:print(json.dumps(prepare(args.dat,args.reference_patch,args.rom,args.out),indent=2))
    except (ValueError,OSError,KeyError,struct.error,zipfile.BadZipFile) as error:
        parser.exit(2,f'Preparation stopped: {error}\n')

if __name__=='__main__':main()
