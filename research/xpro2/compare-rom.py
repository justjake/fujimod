#!/usr/bin/env python3
"""Compare two bounded, existing ROM-window files offline; never access a camera."""
import argparse
import hashlib
import json
from pathlib import Path

REFERENCE='632445259ac679bf36281ea3f707b127759e0802bd507ba2b8d5b926492fd59f'


def read_window(path):
    with path.open('rb') as handle:value=handle.read(32769)
    if len(value)!=32768:raise ValueError('exact 32 KiB files required; do not include peripheral/SRAM tail')
    return value


def compare(first,second):
    if len(first)!=32768 or len(second)!=32768:raise ValueError('exact 32 KiB windows required')
    hashes=[hashlib.sha256(value).hexdigest() for value in [first,second]]
    stable=first==second
    return {'schema':1,'base':'0xfff00000','bytes':32768,'reads_identical':stable,
            'sha256':hashes,'matches_xt2_reference':stable and hashes[0]==REFERENCE,
            'provenance':'Offline supplied files; physical body provenance not established',
            'physical_rom_size_established':False,'physical_boot_qualified':False,'ready_to_flash':False}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('first',type=Path);parser.add_argument('second',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    try:
        value=compare(read_window(args.first),read_window(args.second))
        with args.output.open('x') as handle:json.dump(value,handle,indent=2);handle.write('\n')
        print('Stable window' if value['reads_identical'] else 'Unstable window; stop identification')
    except (ValueError,OSError) as error:parser.exit(2,f'Comparison stopped: {error}\n')

if __name__=='__main__':main()
