#!/usr/bin/env python3
"""Compose nonoverlapping internal theme, F1 and preference patches."""
import os
from pathlib import Path
import argparse
import hashlib
import json

ROOT=Path(os.environ['XPRO2_LAB_ROOT'])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--preferences',type=Path,default=ROOT/'analysis/xpro2-f1-theme-preferences-internal/preferences-patches.json')
    parser.add_argument('--out',type=Path,default=ROOT/'analysis/xpro2-theme-integration-internal')
    args=parser.parse_args()
    out=args.out.resolve()
    assert out.is_relative_to(ROOT/'analysis')
    out.mkdir(exist_ok=True)
    render_dir=ROOT/'analysis/xpro2-theme-render-internal'
    render=json.loads((render_dir/'theme-patches.json').read_text())['patches']
    proof=json.loads((render_dir/'provenance.json').read_text())
    prefs=json.loads(args.preferences.read_text())['patches']
    auxiliary=[p for p in prefs if int(p['address'],0)==0x90800]
    assert len(auxiliary)==1
    core=bytearray(Path(render[0]['file']).read_bytes())
    assert hashlib.sha256(core).hexdigest()==proof['core_sha256']
    assert hashlib.sha256(Path(render[1]['file']).read_bytes()).hexdigest()==proof['sha256']
    data=Path(auxiliary[0]['file']).read_bytes()
    assert len(data)<=0x200 and core[0x800:0x800+len(data)]==bytes(len(data))
    core[0x800:0x800+len(data)]=data
    merged=out/'theme-core-with-menu.bin';merged.write_bytes(core)
    render[0]={**render[0],'file':str(merged)}
    patches=render+[p for p in prefs if p is not auxiliary[0]]
    spans=[]
    for patch in patches:
        start=int(patch['address'],0)
        size=Path(patch['file']).stat().st_size if 'file' in patch else 4
        assert all(start+size<=a or b<=start for a,b in spans),'Overlapping declared patches'
        spans.append((start,start+size))
    (out/'theme-patches.json').write_text(json.dumps({'patches':patches},indent=2)+'\n')
    (out/'provenance.json').write_text(json.dumps({'schema':1,'core_sha256':hashlib.sha256(core).hexdigest(),
        'patches':len(patches),'menu_auxiliary_merged':True,'overlaps':False,
        'preference_source_sha256':hashlib.sha256(args.preferences.read_bytes()).hexdigest(),
        'limitations':['Internal composition, not live boot or installable DAT qualification']},indent=2)+'\n')
    print('COMPOSED',len(patches),'nonoverlapping internal patches')


if __name__=='__main__':main()
