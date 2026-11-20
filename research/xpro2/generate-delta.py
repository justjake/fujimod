#!/usr/bin/env python3
"""Regenerate the sparse recipe from local, hash-checked research artifacts."""
import argparse
from pathlib import Path
from prepare import digest,mod_reference,require,SD_SHA

STOCK_SHA='ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172'


def generate(stock,modified,reference):
    require(digest(stock)==STOCK_SHA and digest(modified)==SD_SHA,'stock/modified identity mismatch')
    ranges=[];start=None;last=None
    for i,(a,b) in enumerate(zip(stock,modified)):
        if a!=b:
            if start is None:start=i
            if last is not None and i-last>32:ranges.append((start,last+1));start=i
            last=i
    if start is not None:ranges.append((start,last+1))
    lines=['# Generated sparse patch source for the qualified FujiMods SD image.',
           '# Apply to the SHA-bound stock image, never to flash or to a DAT file.',
           '# External reference bytes use incbin; no reference module is redistributed.']
    for a,b in ranges:
        lines.append(f'.org 0x{a:x}');pos=a;pending=bytearray()
        def flush():
            for at in range(0,len(pending),16):
                lines.append('.byte '+','.join(f'0x{x:02x}' for x in pending[at:at+16]))
            pending.clear()
        while pos<b:
            found=None
            if b-pos>=16:
                at=reference.find(modified[pos:pos+16])
                if at>=0:
                    size=16
                    while pos+size<b and at+size<len(reference) and modified[pos+size]==reference[at+size]:size+=1
                    if any(modified[pos:pos+size]):found=(at,size)
            if found:
                flush();at,size=found
                lines.append(f'.incbin "mod-reference", 0x{at:x}, 0x{size:x}');pos+=size
            else:pending.append(modified[pos]);pos+=1
        flush()
    return '\n'.join(lines)+'\n'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--stock',type=Path,required=True)
    parser.add_argument('--modified',type=Path,required=True)
    parser.add_argument('--reference-patch',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    text=generate(args.stock.read_bytes(),args.modified.read_bytes(),mod_reference(args.reference_patch.read_bytes()))
    with args.output.open('x') as handle:handle.write(text)
    print('Generated sparse source; third-party input bytes remain external')

if __name__=='__main__':main()
