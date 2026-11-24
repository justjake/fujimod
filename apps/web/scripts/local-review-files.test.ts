import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { build } from 'vite';
import {
  assertNoPublicReviews,
  localReviewFiles,
  reviewFiles,
  staticFiles,
} from './local-review-files.ts';

await test('external reviews stay local, support media ranges and cannot escape their root', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'reviews-test-'));
  const repository = join(dir, 'repository');
  const reviews = join(dir, 'reviews');
  await mkdir(repository);
  await mkdir(join(reviews, 'demo'), { recursive: true });
  await writeFile(join(reviews, 'demo/index.html'), '<h1>Local review</h1>');
  await writeFile(join(reviews, 'demo/clip.mp4'), '0123456789');
  await writeFile(join(repository, 'secret.txt'), 'secret');
  await symlink(repository, join(reviews, 'escape'));
  assert.throws(() => reviewFiles(repository, repository), /outside the repository/);
  const middleware = reviewFiles(reviews, repository);
  const server = createServer((req, res) =>
    middleware(req, res, () => {
      res.statusCode = 404;
      res.end();
    }),
  );
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const url = `http://127.0.0.1:${address.port}`;
  try {
    assert.equal(await (await fetch(`${url}/review/demo/`)).text(), '<h1>Local review</h1>');
    const video = await fetch(`${url}/review/demo/clip.mp4`, { headers: { Range: 'bytes=2-4' } });
    assert.equal(video.status, 206);
    assert.equal(video.headers.get('Content-Type'), 'video/mp4');
    assert.equal(await video.text(), '234');
    assert.equal((await fetch(`${url}/review/demo/missing`)).status, 404);
    assert.equal((await fetch(`${url}/review/escape/secret.txt`)).status, 403);
    assert.equal((await fetch(`${url}/review/%2e%2e%2frepository/secret.txt`)).status, 403);
    assert.equal((await fetch(`${url}/review/demo/clip.mp4`, { method: 'POST' })).status, 405);
    assertNoPublicReviews(repository);
    await mkdir(join(repository, 'public/review'), { recursive: true });
    assert.throws(() => assertNoPublicReviews(repository), /outside the repository/);
  } finally {
    await new Promise<void>((done, fail) =>
      server.close((error) => (error ? fail(error) : done())),
    );
    await rm(dir, { recursive: true });
  }
});

await test('production builds never copy external reviews and reject public review files', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'review-build-test-'));
  const repository = join(dir, 'repository');
  const reviews = join(dir, 'reviews');
  await mkdir(repository);
  await mkdir(reviews);
  await writeFile(join(repository, 'index.html'), '<!doctype html><title>App</title>');
  await writeFile(join(reviews, 'secret.txt'), 'local only');
  const options = {
    configFile: false as const,
    root: repository,
    plugins: [localReviewFiles(reviews)],
    logLevel: 'silent' as const,
  };
  try {
    await build(options);
    assert.deepEqual(await readdir(join(repository, 'dist')), ['index.html']);
    await mkdir(join(repository, 'public/review'), { recursive: true });
    await writeFile(join(repository, 'public/review/secret.txt'), 'must not deploy');
    await assert.rejects(build(options), /Local review files must live outside the repository/);
  } finally {
    await rm(dir, { recursive: true });
  }
});

await test('a filtered mount passes other methods on for paths it does not serve', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'reviews-filtered-'));
  await writeFile(join(dir, 'page.png'), 'png');
  const middleware = staticFiles(dir, '/', (path) => path.endsWith('.png'));
  const server = createServer((req, res) =>
    middleware(req, res, () => {
      res.statusCode = 204;
      res.end();
    }),
  );
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const url = `http://127.0.0.1:${address.port}`;
  try {
    assert.equal((await fetch(`${url}/__report`, { method: 'POST' })).status, 204);
    assert.equal((await fetch(`${url}/page.png`, { method: 'POST' })).status, 405);
  } finally {
    await new Promise<void>((done, fail) =>
      server.close((error) => (error ? fail(error) : done())),
    );
    await rm(dir, { recursive: true });
  }
});
