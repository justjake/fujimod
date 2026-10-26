import { test, vi } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';
import { loadPreparedBundle } from './prepared-bundle';

const manifest = {
  document: 'document.json',
  bootRom: 'boot.bin',
  nand: 'nand.gz',
  settings: 'settings.bin',
};
const base = new URL('https://example.invalid/runtime/wasm.json');
test('fresh cameras need only the prepared document and boot ROM', async () => {
  vi.stubGlobal(
    'fetch',
    async (url: URL) =>
      new Response(url.pathname.endsWith('document.json') ? '{"schema":1}' : Uint8Array.of(7, 8)),
  );
  try {
    const loaded = await loadPreparedBundle(
      { document: 'document.json', bootRom: 'boot.bin' },
      base,
    );
    assert.equal(loaded.inputs.nand, undefined);
    assert.equal(loaded.inputs.settings, undefined);
    assert.equal(loaded.inputs.camera, undefined);
    assert.deepEqual(loaded.inputs.bootRom, Uint8Array.of(7, 8));
  } finally {
    vi.unstubAllGlobals();
  }
});
test('prepared bundle inflates NAND and preserves boot ROM and MCU bytes', async () => {
  const nand = Uint8Array.from([255, 0, 1, 2, 255]);
  const files = new Map<string, string | Uint8Array>([
    ['document.json', '{"schema":1}'],
    ['boot.bin', Uint8Array.of(7, 8)],
    ['nand.gz', gzipSync(nand)],
    ['settings.bin', Uint8Array.of(9, 10)],
  ]);
  const urls: string[] = [];
  vi.stubGlobal('fetch', async (url: URL) => {
    urls.push(url.href);
    const body = files.get(url.pathname.split('/').at(-1)!);
    assert.ok(body !== undefined);
    return new Response(typeof body === 'string' ? body : new Uint8Array(body));
  });
  try {
    const loaded = await loadPreparedBundle(manifest, base);
    assert.equal(loaded.text, '{"schema":1}');
    assert.deepEqual(loaded.inputs.bootRom, Uint8Array.of(7, 8));
    assert.deepEqual(loaded.inputs.nand, nand);
    assert.deepEqual(loaded.inputs.settings, Uint8Array.of(9, 10));
    assert.equal(urls.length, 4);
    assert.ok(urls.every((url) => url.startsWith('https://example.invalid/runtime/')));
  } finally {
    vi.unstubAllGlobals();
  }
});
test('prepared bundle passes camera sections through compressed', async () => {
  const files = new Map<string, string | Uint8Array>([
    ['document.json', '{"schema":1}'],
    ['boot.bin', Uint8Array.of(7, 8)],
    ['nand-configuration.bin.zst', Uint8Array.of(0x28, 0xb5, 0x2f, 0xfd, 1)],
    ['nand-tone-curves.bin.zst', Uint8Array.of(0x28, 0xb5, 0x2f, 0xfd, 2)],
  ]);
  vi.stubGlobal('fetch', async (url: URL) => {
    const body = files.get(url.pathname.split('/').at(-1)!);
    return body === undefined
      ? new Response(null, { status: 404 })
      : new Response(typeof body === 'string' ? body : new Uint8Array(body));
  });
  try {
    const camera = {
      configuration: 'nand-configuration.bin.zst',
      'tone-curves': 'nand-tone-curves.bin.zst',
    };
    const loaded = await loadPreparedBundle(
      { document: 'document.json', bootRom: 'boot.bin', camera },
      base,
    );
    assert.deepEqual(loaded.inputs.camera, {
      configuration: Uint8Array.of(0x28, 0xb5, 0x2f, 0xfd, 1),
      'tone-curves': Uint8Array.of(0x28, 0xb5, 0x2f, 0xfd, 2),
    });
    await assert.rejects(
      loadPreparedBundle(
        { document: 'document.json', bootRom: 'boot.bin', camera: { configuration: 'absent.zst' } },
        base,
      ),
      /input is unavailable/,
    );
    await assert.rejects(
      loadPreparedBundle(
        { document: 'document.json', bootRom: 'boot.bin', camera: { configuration: 1 } },
        base,
      ),
      /invalid camera configuration input/,
    );
  } finally {
    vi.unstubAllGlobals();
  }
});
test('prepared bundle refuses missing media declarations and failed downloads', async () => {
  await assert.rejects(loadPreparedBundle({ document: 'document.json' }, base), /bootRom input/);
  vi.stubGlobal('fetch', async () => new Response(null, { status: 404 }));
  try {
    await assert.rejects(loadPreparedBundle(manifest, base), /input is unavailable/);
  } finally {
    vi.unstubAllGlobals();
  }
});
test('prepared bundle refuses corrupt compressed NAND', async () => {
  vi.stubGlobal('fetch', async () => new Response(Uint8Array.of(1, 2, 3)));
  try {
    await assert.rejects(loadPreparedBundle(manifest, base));
  } finally {
    vi.unstubAllGlobals();
  }
});
