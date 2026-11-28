#!/usr/bin/env python3
"""Verify the historical engine commits without changing a checkout."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

HERE=Path(__file__).resolve().parent
ENGINE_REFERENCE = '136ae193fa3f2366d3a28dba334db427e97f1f62'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo',type=Path,required=True,help='FujiMod Git store containing the engine reference branch')
    args=parser.parse_args();repo=args.repo.resolve()
    meta=json.loads((HERE/'data/provenance.generated.json').read_text())
    hashes=json.loads((HERE/'data/engine-path-hashes.generated.json').read_text())
    git = ['git', '-C', str(repo)]
    def read(*command):
        result = subprocess.run(git + list(command), capture_output=True)
        if result.returncode:
            parser.error(result.stderr.decode(errors='replace').strip())
        return result.stdout

    if read('rev-parse', f'{ENGINE_REFERENCE}~3').decode().strip() != meta['engine_base']:
        parser.error('engine reference base differs')
    changed = read('diff', '--name-only', '-z', meta['engine_base'], ENGINE_REFERENCE).decode()
    if set(changed.rstrip('\0').split('\0')) != set(hashes):
        parser.error('engine reference path set differs')
    for path, digest in hashes.items():
        relative = Path(path)
        if relative.is_absolute() or '..' in relative.parts:
            parser.error('invalid source path')
        if hashlib.sha256(read('show', f'{ENGINE_REFERENCE}:{path}')).hexdigest() != digest:
            parser.error(f'engine source hash differs: {path}')
    print(f'PASS engine reference commits: {len(hashes)} paths; checkout unchanged')

if __name__=='__main__':main()
