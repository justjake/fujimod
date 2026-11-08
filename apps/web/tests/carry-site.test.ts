import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { carrySite, validateSiteAssets } from '../scripts/carry-site.ts';
import { cameraApi } from '../scripts/runtime-api';

/** Serve `files` as a deployed site with the `site-assets.json` staging would write. */
async function deployedSite(files: Record<string, string>, tamper: Record<string, string> = {}) {
  const manifest = {
    schema: 1,
    files: Object.entries(files).map(([path, data]) => ({
      path,
      bytes: Buffer.byteLength(data),
      sha256: createHash('sha256').update(data).digest('hex'),
    })),
  };
  const served: Record<string, string> = {
    ...files,
    ...tamper,
    'site-assets.json': JSON.stringify(manifest),
  };
  const server = createServer((req, res) => {
    const data =
      served[decodeURIComponent(new URL(req.url ?? '/', 'http://site').pathname.slice(1))];
    res.writeHead(data === undefined ? 404 : 200).end(data);
  });
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  return {
    origin: `http://127.0.0.1:${(server.address() as AddressInfo).port}/`,
    close: () => new Promise<void>((done) => server.close(() => done())),
  };
}

test('site asset manifests name only staged paths with sizes and hashes', () => {
  const file = { path: 'runtime/catalog.json', bytes: 2, sha256: 'a'.repeat(64) };
  assert.deepEqual(validateSiteAssets({ schema: 1, files: [file] }), [file]);
  assert.throws(() => validateSiteAssets({ schema: 2, files: [file] }), /unknown schema/);
  assert.throws(() => validateSiteAssets({ schema: 1, files: [] }), /no files/);
  for (const path of ['index.html', 'runtime/../worker.ts', 'artifacts/docs/../../x', '/runtime/a'])
    assert.throws(() => validateSiteAssets({ schema: 1, files: [{ ...file, path }] }), /path/);
  assert.throws(() => validateSiteAssets({ schema: 1, files: [file, file] }), /Duplicate/);
  assert.throws(() => validateSiteAssets({ schema: 1, files: [{ ...file, bytes: 0 }] }), /size/);
  assert.throws(() => validateSiteAssets({ schema: 1, files: [{ ...file, sha256: 'x' }] }), /hash/);
});

test('carrying a deployed site verifies every asset and the runtime Camera API', async () => {
  const declarations = await readFile(
    new URL('../../../bindings/javascript/emulator/camera.generated.ts', import.meta.url),
    'utf8',
  );
  const loader = `class Camera { ${[...cameraApi(declarations)].map((method) => `${method}() {}`).join(' ')} }`;
  const video = 'a'.repeat(24) + '.mp4',
    still = 'a'.repeat(24) + '.webp';
  const files: Record<string, string> = {
    'runtime/catalog.json': JSON.stringify({
      schema: 1,
      firmwares: [
        {
          id: 'test-100',
          body: 'test',
          model: 'Test',
          version: '1.00',
          sha256: '1'.repeat(64),
          bytes: 42,
          release_date: '',
          download_url: 'https://example.org/firmware',
          emulate: { bundle: 'runtime/test-100/', sceneInput: false },
        },
      ],
    }),
    'runtime/test-100/camera.mjs': loader,
    'runtime/test-100/camera.wasm': 'wasm',
    'runtime/test-100/licenses/COPYING.txt': 'license',
    'artifacts/docs/media.json': JSON.stringify({
      schema: 1,
      cards: { card: { video, still, clips: {} } },
    }),
    [`artifacts/docs/${video}`]: 'video',
    [`artifacts/docs/${still}`]: 'still',
  };
  const root = await mkdtemp(join(tmpdir(), 'carry-site-'));
  const destination = join(root, 'public');
  const tampered = await deployedSite(files, { 'runtime/test-100/camera.wasm': 'wasn' });
  const outdated = await deployedSite({
    ...files,
    'runtime/test-100/camera.mjs': 'class Camera {}',
  });
  const deployed = await deployedSite(files);
  try {
    await assert.rejects(carrySite(tampered.origin, destination), /differs from its manifest/);
    await assert.rejects(carrySite(outdated.origin, destination), /Runtime Camera API is missing/);
    await assert.rejects(readFile(join(destination, 'runtime/catalog.json')), /ENOENT/);
    assert.deepEqual(await carrySite(deployed.origin, destination), { emulators: 1, files: 7 });
    for (const [path, data] of Object.entries(files))
      assert.equal(await readFile(join(destination, path), 'utf8'), data);
    const manifest = JSON.parse(await readFile(join(destination, 'site-assets.json'), 'utf8')) as {
      files: { path: string }[];
    };
    assert.equal(manifest.files.length, 7);
  } finally {
    await Promise.all([tampered.close(), outdated.close(), deployed.close()]);
    await rm(root, { recursive: true, force: true });
  }
});
