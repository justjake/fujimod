#!/usr/bin/env python3
"""Check the allowlisted publication tree; never scan or upload private inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import re

HERE=Path(__file__).resolve().parent
ALLOWED={'.py','.md','.json','.patch','.S','.txt','.toml'}
PATTERNS={
    'absolute personal directory':r'''(?:^|[\s"'=])/(?:home|Users)/[^\s"']+''',
    'Windows personal directory':r'[A-Za-z]:\\(?:Users|Documents and Settings)\\',
    'private lab path':r'Desktop/' + r'codex|firmware_' + r'test|\.venv-' + r'emu/lib/python',
    'private key':r'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----',
    'GitHub token':r'\b(?:ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})',
    'Slack token':r'\bxox[baprs]-[A-Za-z0-9-]{20,}',
}


def source_files():
    return sorted(p for p in HERE.rglob('*') if p.is_file() and '__pycache__' not in p.parts)


def inspect():
    files={};failures=[]
    for path in source_files():
        name=path.relative_to(HERE).as_posix();raw=path.read_bytes()
        if (path.suffix not in ALLOWED and path.name!='LICENSE') or b'\0' in raw or len(raw)>1048576:
            failures.append((name,'unexpected file type, binary data or oversized file'));continue
        try:text=raw.decode('utf-8')
        except UnicodeDecodeError:failures.append((name,'not UTF-8 source text'));continue
        for description,pattern in PATTERNS.items():
            if re.search(pattern,text,re.M):failures.append((name,description))
        if path.suffix=='.json':
            value=json.loads(text)
            def visit(v,key=''):
                if isinstance(v,dict):
                    for k,x in v.items():visit(x,k)
                elif isinstance(v,list):
                    if key in {'bytes','data','payload'} and len(v)>128 and all(isinstance(x,int) and 0<=x<=255 for x in v):
                        failures.append((name,'large binary array in JSON'))
                    for x in v:visit(x,key)
                elif isinstance(v,str) and key.lower() in {'datahex','payloadhex','base64'} and len(v)>256:
                    failures.append((name,'large binary string in JSON'))
            visit(value)
        files[name]={'sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw)}
    if failures:raise ValueError('\n'.join(f'{name}: {reason}' for name,reason in failures))
    return files


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,help='Optional external manifest; must be outside this tree')
    args=parser.parse_args()
    files=inspect()
    if args.manifest:
        if args.manifest.resolve().is_relative_to(HERE):parser.error('write the audit manifest outside its own source tree')
        with args.manifest.open('x') as handle:json.dump({'status':'pass','files':files},handle,indent=2);handle.write('\n')
    print(f'PASS publication audit: {len(files)} UTF-8 source files; no personal paths, recognized credentials or binary inputs')

if __name__=='__main__':main()
