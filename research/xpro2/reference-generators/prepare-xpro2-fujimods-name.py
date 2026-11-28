#!/usr/bin/env python3
"""Rename the owned menu caption; preserve all feature code and preferences."""
import hashlib
import json
import os
from pathlib import Path
import struct

ROOT = Path(os.environ['XPRO2_LAB_ROOT'])
OUT = ROOT / 'analysis/xpro2-fujimods-name'
LABEL = 'FujiMods'


def main():
    source = ROOT/'analysis/xpro2-framing-internal/patches.json'
    patches = json.loads(source.read_text())
    module = next(p for p in patches['patches'] if int(p['address'], 0) == 0xa9000)
    raw = Path(module['file']).read_bytes()
    before = hashlib.sha256(raw).hexdigest()
    blob = bytearray(raw)
    pointer_offset = 0x600 + 7*4
    old = struct.unpack_from('<I', blob, pointer_offset)[0] - 0xa9000
    assert blob[old:old+6] == 'F1\0'.encode('utf-16-le')
    # Reserved text area before resource headers; never overwrite a neighbour.
    target = 0xbe0
    text = (LABEL+'\0').encode('utf-16-le')
    assert target+len(text) <= 0xc00 and not any(blob[target:0xc00])
    blob[target:target+len(text)] = text
    struct.pack_into('<I', blob, pointer_offset, 0xa9000+target)
    changed = [i for i, (a,b) in enumerate(zip(raw, blob)) if a != b]
    assert all(pointer_offset <= i < pointer_offset+4 or target <= i < target+len(text) for i in changed)
    OUT.mkdir(exist_ok=False)
    output = OUT/'framing-module-fujimods.bin'
    output.write_bytes(blob)
    module['file'] = str(output)
    module['what'] += '; custom folder caption FujiMods'
    (OUT/'patches.json').write_text(json.dumps(patches, indent=2)+'\n')
    (OUT/'provenance.json').write_text(json.dumps({
        'caption_id': '0x7007', 'old': 'F1', 'new': LABEL,
        'caption_pointer_address': hex(0xa9000+pointer_offset),
        'text_address': hex(0xa9000+target), 'text_bytes': len(text),
        'source_module_sha256': before, 'module_sha256': hashlib.sha256(blob).hexdigest(),
        'changed_module_bytes': len(changed), 'code_changed': False,
        'preference_schema_changed': False, 'physical_hardware_qualified': False,
    }, indent=2)+'\n')
    print('Prepared FujiMods caption:', len(changed), 'changed data bytes; no code changes')


if __name__ == '__main__': main()
