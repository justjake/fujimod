import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { unzipSync } from 'fflate';
import { build } from './build-web-wheel.ts';

test('wheel records authenticate every included Python source and metadata entry', () => {
  const output = mkdtempSync(join(tmpdir(), 'fujimod-wheel-'));
  try {
    const path = build(output),
      first = readFileSync(path),
      files = unzipSync(first);
    assert.deepEqual(readFileSync(build(output)), first, 'Repeated builds are deterministic');
    assert(Object.keys(files).some((name) => name === 'fujimod/__init__.py'));
    assert(Object.keys(files).some((name) => name === 'fujimod/usb/__init__.py'));
    assert(!Object.keys(files).some((name) => name.startsWith('fujimod/emulator/')));
    assert(
      Object.keys(files).every((name) => name.endsWith('.py') || name.includes('.dist-info/')),
    );
    const record = Object.keys(files).find((name) => name.endsWith('.dist-info/RECORD'));
    assert(record);
    const rows = Buffer.from(files[record]).toString().trimEnd().split('\r\n');
    assert.equal(rows.length, Object.keys(files).length);
    for (const row of rows) {
      const [name, hash, size] = row.split(',');
      if (name === record) {
        assert.equal(hash, '');
        assert.equal(size, '');
        continue;
      }
      const data: Uint8Array = files[name];
      assert(data);
      assert.equal(hash, 'sha256=' + createHash('sha256').update(data).digest('base64url'));
      assert.equal(size, String(data.length));
    }
    const metadata = Object.keys(files).find((name) => name.endsWith('/METADATA'));
    assert(metadata);
    assert.match(Buffer.from(files[metadata]).toString(), /Requires-Python: >=3\.14\n/);
    assert.equal(
      JSON.parse(readFileSync(join(output, 'manifest.json'), 'utf8')).wheel,
      path.split('/').at(-1),
    );
  } finally {
    rmSync(output, { recursive: true, force: true });
  }
});
