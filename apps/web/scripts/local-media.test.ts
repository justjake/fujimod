import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rename, rm, symlink, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createServer as createViteServer } from 'vite';
import { localMedia, previewMedia, recordingFiles } from './local-media.ts';
import { staticFiles } from './local-review-files.ts';

await test('frontier, current, promoted and output media use filesystem paths and byte ranges', async () => {
  const root = await mkdtemp(join(tmpdir(), 'media-test-'));
  for (const path of ['media/builds/one/site', 'media/frontier/site', '.tools/run/m/hero/test'])
    await mkdir(join(root, path), { recursive: true });
  await writeFile(join(root, 'media/builds/one/site/media.json'), '{"cards":{"old":{}}}');
  await writeFile(join(root, 'media/frontier/site/media.json'), '{"cards":{"new":{}}}');
  await writeFile(join(root, 'media/frontier/site/video.mp4'), '0123456789');
  await writeFile(join(root, '.tools/run/m/hero/test/guide.json'), '{}');
  await writeFile(join(root, '.tools/run/m/hero/test/video.mp4'), 'recording');
  await mkdir(join(root, '.tools/run/.inputs'), { recursive: true });
  await writeFile(join(root, '.tools/run/.inputs/receipt.json'), '{}');
  await writeFile(join(root, '.tools/run/.inputs/secret.json'), 'private');
  await writeFile(join(root, 'source.json'), 'source');
  await symlink('builds/one/site', join(root, 'media/current'));
  await symlink(join(root, 'source.json'), join(root, 'media/frontier/site/escape.json'));
  const files = recordingFiles(root);
  const server = createServer((req, res) => {
    const missing = () => {
      res.statusCode = 404;
      res.end();
    };
    if (req.url?.startsWith('/artifacts/docs/')) {
      const selected = previewMedia(root, undefined);
      if (selected) return staticFiles(selected, '/artifacts/docs/')(req, res, missing);
    }
    files(req, res, missing);
  });
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const url = `http://127.0.0.1:${address.port}`;
  try {
    assert.equal(previewMedia(root, undefined), join(root, 'media/frontier/site'));
    const video = await fetch(`${url}/media/frontier/site/video.mp4`, {
      headers: { Range: 'bytes=2-4' },
    });
    assert.equal(video.status, 206);
    assert.equal(video.headers.get('content-type'), 'video/mp4');
    assert.equal(await video.text(), '234');
    for (const path of ['media/current/media.json', 'media/builds/one/site/media.json'])
      assert.equal((await fetch(`${url}/${path}`)).status, 200);
    assert.equal(
      await (await fetch(`${url}/.tools/run/m/hero/test/video.mp4`)).text(),
      'recording',
    );
    for (const path of ['.tools/run/.inputs/secret.json', 'source.json'])
      assert.equal((await fetch(`${url}/${path}`)).status, 404);
    assert.equal((await fetch(`${url}/media/frontier/site/escape.json`)).status, 403);
    assert.equal((await fetch(`${url}/media/frontier/site/%2e%2e%2fsource.json`)).status, 403);
    assert.equal(
      await (await fetch(`${url}/artifacts/docs/media.json`)).text(),
      '{"cards":{"new":{}}}',
    );
    await rename(join(root, 'media/frontier'), join(root, 'media/builds/two'));
    assert.equal(previewMedia(root, undefined), join(root, 'media/current'));
    assert.equal(
      await (await fetch(`${url}/artifacts/docs/media.json`)).text(),
      '{"cards":{"old":{}}}',
    );
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
});

await test('a landed manifest refreshes the development homepage module', async () => {
  const root = await mkdtemp(join(tmpdir(), 'media-vite-test-'));
  await mkdir(join(root, 'media/frontier/site'), { recursive: true });
  const manifest = join(root, 'media/frontier/site/media.json');
  await writeFile(manifest, '{"cards":{"before":{}}}');
  await writeFile(join(root, 'entry.ts'), 'export const media = __TEST_MEDIA__;');
  const server = await createViteServer({
    configFile: false,
    root,
    logLevel: 'silent',
    server: { host: '127.0.0.1', port: 0 },
    plugins: [
      {
        name: 'homepage-manifest',
        config: () => ({ define: { __TEST_MEDIA__: readFileSync(manifest, 'utf8') } }),
      },
      localMedia(root, undefined),
    ],
  });
  try {
    await server.listen();
    assert.match(String(server.config.define?.__TEST_MEDIA__), /before/);
    await writeFile(join(root, 'media/frontier/site/.media.json.tmp'), '{"cards":{"after":{}}}');
    await rename(join(root, 'media/frontier/site/.media.json.tmp'), manifest);
    const deadline = Date.now() + 10000;
    let code = '';
    while (Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
      code = String(server.config.define?.__TEST_MEDIA__);
      if (code.includes('after')) break;
    }
    assert.match(code, /after/);
  } finally {
    await server.close();
    await rm(root, { recursive: true, force: true });
  }
});
