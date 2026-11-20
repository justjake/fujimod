#!/usr/bin/env python3
"""Check forward/reverse historical source replay without changing a checkout."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

HERE=Path(__file__).resolve().parent


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo',type=Path,required=True,help='FujiMod Git store containing the historical base')
    args=parser.parse_args();repo=args.repo.resolve()
    meta=json.loads((HERE/'data/provenance.generated.json').read_text())
    hashes=json.loads((HERE/'data/engine-path-hashes.generated.json').read_text())
    patch=HERE/'legacy/engine-reference.generated.patch'
    if hashlib.sha256(patch.read_bytes()).hexdigest()!=meta['legacy_engine_patch_sha256']:
        parser.error('engine patch identity mismatch')
    original={}
    with tempfile.TemporaryDirectory(prefix='xpro2-source-replay-') as directory:
        target=Path(directory)
        for path in hashes:
            relative=Path(path)
            if relative.is_absolute() or '..'in relative.parts:parser.error('invalid source path')
            result=subprocess.run(['git','-C',str(repo),'show',meta['engine_base']+':'+path],capture_output=True)
            original[path]=None if result.returncode else result.stdout
            if result.returncode==0:
                out=target/path;out.parent.mkdir(parents=True,exist_ok=True);out.write_bytes(result.stdout)
        for flags in [['--check'],[]]:subprocess.run(['git','apply',*flags,str(patch)],cwd=target,check=True,capture_output=True)
        for path,digest in hashes.items():
            assert hashlib.sha256((target/path).read_bytes()).hexdigest()==digest,path
        subprocess.run(['git','apply','--reverse',str(patch)],cwd=target,check=True,capture_output=True)
        for path,data in original.items():
            out=target/path
            assert (not out.exists()) if data is None else out.read_bytes()==data,path
    print(f'PASS forward and reverse source replay: {len(hashes)} paths; checkout unchanged')

if __name__=='__main__':main()
