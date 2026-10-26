import { expect, test } from 'vite-plus/test';
import { inflateRawSync } from 'node:zlib';
import { execFileSync } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { crc32, zipBlob } from './archive';

test('deflate ZIP preserves UTF-8 names, CRCs and large Blob contents and passes unzip -t', async () => {
  const block = new Uint8Array(1024 * 1024).fill(42);
  const large = new Blob(Array.from({ length: 128 }, () => block));
  // The source must be read through its stream, without materializing its arrayBuffer.
  large.arrayBuffer = () => {
    throw Error('Full source read');
  };
  const zip = await zipBlob([
    { path: 'camera/flash.bin', data: large },
    { path: 'café/empty', data: new Blob() },
  ]);
  expect(zip.size).toBeLessThan(1024 * 1024);
  const bytes = new Uint8Array(await zip.arrayBuffer()),
    view = new DataView(bytes.buffer);
  expect(view.getUint16(8, true)).toBe(8);
  const start = 30 + view.getUint16(26, true) + view.getUint16(28, true);
  const decoded = inflateRawSync(bytes.subarray(start, start + view.getUint32(18, true)));
  expect(decoded.length).toBe(large.size);
  expect(crc32(decoded)).toBe(view.getUint32(14, true));
  expect(crc32(new TextEncoder().encode('123456789'))).toBe(0xcbf43926);
  const root = await mkdtemp(join(tmpdir(), 'fujimod-zip-'));
  try {
    const path = join(root, 'archive.zip');
    await writeFile(path, bytes);
    expect(execFileSync('unzip', ['-t', path], { encoding: 'utf8' })).toContain(
      'No errors detected',
    );
  } finally {
    await rm(root, { recursive: true });
  }
}, 30000);
test('ZIP64 directory counts at the 16-bit boundary pass unzip', async () => {
  const empty = new Blob();
  const zip = await zipBlob(
    Array.from({ length: 65535 }, (_, i) => ({ path: `${i}`, data: empty })),
  );
  const bytes = new Uint8Array(await zip.arrayBuffer()),
    view = new DataView(bytes.buffer);
  const end64 = bytes.length - 22 - 20 - 56;
  expect(view.getUint32(end64, true)).toBe(0x06064b50);
  expect(view.getBigUint64(end64 + 32, true)).toBe(65535n);
  expect(view.getUint32(bytes.length - 42, true)).toBe(0x07064b50);
  const root = await mkdtemp(join(tmpdir(), 'fujimod-zip64-'));
  try {
    const path = join(root, 'archive.zip');
    await writeFile(path, bytes);
    execFileSync('unzip', ['-tqq', path]);
  } finally {
    await rm(root, { recursive: true });
  }
}, 30000);
test('empty archives and oversized paths', async () => {
  expect((await zipBlob([])).size).toBe(22);
  await expect(zipBlob([{ path: 'x'.repeat(65536), data: new Blob() }])).rejects.toThrow('UTF-8');
});
