import { inflateRawSync } from 'node:zlib';
import type { PatchMetadata, FileIdentity } from '../src/patch/patcher.ts';
import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { patch, sha256, inspectPatch } from '../src/patch/patcher.ts';
import { validateIndex } from '../src/patch/catalog.ts';
import { cardZip } from '../src/patch/download.ts';
import { crc32 } from '@fujimod/emulator-ui/archive';
const text = (value: string) => new TextEncoder().encode(value);
const concat = (...parts: Uint8Array[]) => {
  const data = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    data.set(part, at);
    at += part.length;
  }
  return data;
};
function op(
  kind: number,
  target: number,
  dest: number,
  length: number,
  source = 0,
  packed = 0,
  flags = 0,
  data = new Uint8Array(),
) {
  const header = new Uint8Array(20),
    view = new DataView(header.buffer);
  header.set([kind, target, flags, 0]);
  for (const [i, n] of [dest, length, source, packed].entries()) view.setUint32(4 + i * 4, n, true);
  return concat(header, data);
}
function lz(tokens = [1, 65, 128, 19], length = 4) {
  const header = new Uint8Array(20),
    view = new DataView(header.buffer);
  for (const [i, n] of [length, 20 + tokens.length, 1, 0x4000, 4].entries())
    view.setUint32(i * 4, n, true);
  return concat(header, new Uint8Array(tokens));
}
async function encode(
  base: Uint8Array,
  commands: Uint8Array[],
  expected: Record<string, Uint8Array>,
  edit: (meta: PatchMetadata) => PatchMetadata = (meta) => meta,
) {
  const program = concat(...commands);
  const outputs: Record<string, FileIdentity> = {};
  for (const [name, data] of Object.entries(expected))
    outputs[name] = { bytes: data.length, sha256: await sha256(data) };
  const meta = edit({
    schema: 1,
    source: { bytes: base.length, sha256: await sha256(base) },
    outputs: outputs as PatchMetadata['outputs'],
    commands: commands.length,
    program_sha256: await sha256(program),
  });
  const json = text(JSON.stringify(meta)),
    header = new Uint8Array(12);
  header.set(text('FJMODP01'));
  new DataView(header.buffer).setUint32(8, json.length, true);
  return concat(header, json, program);
}
async function fixture() {
  const stream = lz(),
    base = concat(
      text('abc'),
      stream.map((n) => n ^ 255),
    );
  const commands = [
    op(1, 0, 0, 3, 0, 3),
    op(2, 0, 3, 4, 3, stream.length, 1),
    op(3, 0, 8, 2, 0, 0, 0, text('XY')),
    op(1, 1, 0, 3, 0, 3),
    op(3, 1, 3, 1, 0, 0, 0, text('!')),
  ];
  const expected = {
    ffboot: concat(text('abcAAAA'), new Uint8Array(1), text('XY'), new Uint8Array(2)),
    ffsys: text('abc!'),
  };
  return { base, commands, expected, compiled: await encode(base, commands, expected) };
}
test('COPY, XOR, overlapping Fuji LZ, WRITE, zero fill and both output hashes', async () => {
  const f = await fixture(),
    result = await patch(f.base, f.compiled);
  assert.deepEqual(result.ffboot, f.expected.ffboot);
  assert.deepEqual(result.ffsys, f.expected.ffsys);
  assert.equal(result.metadata.schema, 1);
});
test('Blob and offset Uint8Array inputs preserve their exact byte ranges', async () => {
  const f = await fixture(),
    padded = concat(text('padding'), f.compiled, text('tail'));
  const result = await patch(new Blob([f.base]), padded.subarray(7, padded.length - 4));
  assert.deepEqual(result.ffboot, f.expected.ffboot);
});
test('rejects wrong DAT and changed patch bytes', async () => {
  const f = await fixture(),
    changed = f.compiled.slice();
  changed[changed.length - 1] ^= 1;
  await assert.rejects(patch(text('different'), f.compiled), /Stock DAT/);
  await assert.rejects(patch(f.base, changed), /program hash/);
});
test('rejects wrong final output hash, metadata version and oversized allocation', async () => {
  const f = await fixture();
  for (const edit of [
    (m: PatchMetadata) => ({
      ...m,
      outputs: { ...m.outputs, ffboot: { ...m.outputs.ffboot, sha256: '0'.repeat(64) } },
    }),
    (m: PatchMetadata) => ({ ...m, schema: 2 }),
    (m: PatchMetadata) => ({
      ...m,
      outputs: { ...m.outputs, ffboot: { ...m.outputs.ffboot, bytes: 2 ** 31 } },
    }),
  ])
    await assert.rejects(patch(f.base, await encode(f.base, f.commands, f.expected, edit)));
});
test('rejects truncated, unknown, out-of-bounds and flag-invalid commands with valid program hashes', async () => {
  const f = await fixture();
  for (const bad of [
    op(99, 0, 0, 1),
    op(1, 2, 0, 1, 0, 1),
    op(1, 0, 12, 1, 0, 1),
    op(1, 0, 0, 1, 10000, 1),
    op(1, 0, 0, 1, 0, 2),
    op(1, 0, 0, 0, 0, 1),
    op(1, 0, 0, 1, 0, 1, 4),
    op(3, 0, 0, 4, 0, 0, 0, text('x')),
    new Uint8Array(19),
  ])
    await assert.rejects(patch(f.base, await encode(f.base, [bad], f.expected)));
  await assert.rejects(
    patch(
      f.base,
      await encode(f.base, [...f.commands, text('extra')], f.expected, (m) => ({
        ...m,
        commands: f.commands.length,
      })),
    ),
    /Trailing/,
  );
});
test('rejects malformed LZ tokens rather than accepting short or unbounded output', async () => {
  const f = await fixture();
  for (const stream of [
    lz([0]),
    lz([128]),
    lz([128, 19]),
    lz([1, 65, 128, 16]),
    lz([1, 65], 4),
    lz([1, 65, 128, 31], 4),
  ]) {
    const base = concat(text('abc'), stream);
    await assert.rejects(
      patch(base, await encode(base, [op(2, 0, 0, 4, 3, stream.length)], f.expected)),
      /LZ/,
    );
  }
});
test('rejects unsupported magic and invalid metadata framing', async () => {
  const f = await fixture();
  assert.throws(() => inspectPatch(text('bad')));
  const bad = f.compiled.slice();
  new DataView(bad.buffer).setUint32(8, 0xffffffff, true);
  assert.throws(() => inspectPatch(bad), /metadata length/);
});
function index() {
  const digest = 'a'.repeat(64);
  return {
    schema: 1,
    release: 'test',
    patches: { [digest]: 'x70-110.fmpatch' },
    firmwares: [
      {
        boot_instructions: ['Copy files to your card.'],
        id: 'x70-110',
        model: 'X70',
        version: '1.10',
        release_date: '2016-06-09',
        sha256: digest,
        bytes: 100,
        patch_bytes: 50,
        patch_sha256: 'b'.repeat(64),
        download_url: 'https://www.fujifilm-x.com/firmware/',
      },
    ],
  };
}
test('validates hash-to-filename catalog and rejects path traversal, missing maps and unsafe links', () => {
  assert.equal(validateIndex(index()).release, 'test');
  for (const filename of [
    '../bad.fmpatch',
    'https://evil.test/a.fmpatch',
    'bad.fmpatch?query',
    '/root.fmpatch',
  ]) {
    const bad = index();
    bad.patches['a'.repeat(64)] = filename;
    assert.throws(() => validateIndex(bad));
  }
  for (const value of ['2016-02-30', 'not-a-date', null]) {
    const bad = index();
    bad.firmwares[0].release_date = value as string;
    assert.throws(() => validateIndex(bad));
  }
  const absent = index();
  absent.patches = {};
  assert.throws(() => validateIndex(absent));
  const link = index();
  link.firmwares[0].download_url = 'javascript:alert(1)';
  assert.throws(() => validateIndex(link));
  const duplicate = index();
  duplicate.firmwares.push(duplicate.firmwares[0]);
  assert.throws(() => validateIndex(duplicate));
});
test('ZIP has known CRC32, correct file names and CRCs, data and directory offsets', async () => {
  assert.equal(crc32(text('123456789')), 0xcbf43926);
  const expected = { ffboot: text('boot bytes'), ffsys: text('sys bytes') };
  const archive = new Uint8Array(await (await cardZip(expected)).arrayBuffer()),
    view = new DataView(archive.buffer);
  let cursor = 0;
  for (const name of ['ffboot', 'ffsys'] as const) {
    assert.equal(view.getUint32(cursor, true), 0x04034b50);
    const count = view.getUint16(cursor + 26, true),
      length = view.getUint32(cursor + 18, true);
    assert.equal(
      new TextDecoder().decode(archive.subarray(cursor + 30, cursor + 30 + count)),
      name + '.bin',
    );
    assert.equal(view.getUint32(cursor + 14, true), crc32(expected[name]));
    assert.equal(view.getUint16(cursor + 8, true), 8);
    assert.deepEqual(
      Uint8Array.from(
        inflateRawSync(archive.subarray(cursor + 30 + count, cursor + 30 + count + length)),
      ),
      expected[name],
    );
    cursor += 30 + count + length;
  }
  assert.equal(view.getUint32(archive.length - 22, true), 0x06054b50);
  assert.equal(view.getUint32(archive.length - 6, true), cursor);
});

test('new firmware entries extend the catalog without changing the page', () => {
  const catalog = index(),
    next = {
      ...catalog.firmwares[0],
      id: 'next-firmware',
      model: 'Another model',
      sha256: 'c'.repeat(64),
    };
  catalog.firmwares.push(next);
  catalog.patches[next.sha256] = 'next-firmware.fmpatch';
  assert.equal(validateIndex(catalog).firmwares.length, 2);
});

test('optional FP update output is reconstructed and hash checked', async () => {
  const base = text('stock');
  const expected = { ffboot: text('boot'), ffsys: text('system'), fpupdate: text('update') };
  const commands = Object.values(expected).map((data, target) =>
    op(3, target, 0, data.length, 0, 0, 0, data),
  );
  const compiled = await encode(base, commands, expected);
  const result = await patch(base, compiled);
  assert.deepEqual(result.fpupdate, expected.fpupdate);
  const corrupt = await encode(base, commands, { ...expected, fpupdate: text('broken') });
  await assert.rejects(patch(base, corrupt), /output hash/);
});
