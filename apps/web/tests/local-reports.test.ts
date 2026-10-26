import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { zipBlob } from '@fujimod/emulator-ui/archive';
import { localReports, simulatorReports } from '../scripts/local-reports.ts';

test('local report writes metadata first and streams ZIP to its own directory', async () => {
  const root = await mkdtemp(join(tmpdir(), 'fujimod-reports-'));
  const middleware = localReports(root, { info() {} });
  const server = createServer((req, res) =>
    middleware(req, res, (error) => {
      res.statusCode = error ? 500 : 404;
      res.end();
    }),
  );
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  const endpoint = `http://127.0.0.1:${address.port}/__simulator-reports`;
  const id = randomUUID();
  try {
    const metadata = { id, source: 'dev-tools', event: 'crash', fault: { message: 'Guest fault' } };
    assert.equal(
      (await fetch(endpoint, { method: 'POST', body: JSON.stringify(metadata) })).status,
      201,
    );
    const path = join(root, id),
      json = () => readFile(join(path, 'metadata.json'), 'utf8').then(JSON.parse);
    assert.equal((await json()).archive.status, 'pending');
    const bytes = new Uint8Array(
      await (
        await zipBlob([
          { path: 'report.json', data: new Blob([JSON.stringify({ description: 'Guest fault' })]) },
        ])
      ).arrayBuffer(),
    );
    assert.equal(
      (await fetch(`${endpoint}/${id}/report.zip`, { method: 'PUT', body: bytes })).status,
      201,
    );
    assert.deepEqual(new Uint8Array(await readFile(join(path, 'report.zip'))), bytes);
    assert.equal((await json()).archive.status, 'saved');
    assert.equal((await json()).archive.sha256.length, 64);
    assert.deepEqual((await readdir(path)).sort(), ['metadata.json', 'report.json', 'report.zip']);
    assert.equal(
      (await fetch(endpoint, { method: 'POST', body: JSON.stringify(metadata) })).status,
      409,
    );
    assert.equal(
      (
        await fetch(endpoint, {
          method: 'POST',
          headers: { origin: 'https://other.invalid' },
          body: JSON.stringify(metadata),
        })
      ).status,
      403,
    );
    assert.equal(
      (
        await fetch(endpoint, {
          method: 'POST',
          body: JSON.stringify({ ...metadata, id: '../escape' }),
        })
      ).status,
      400,
    );
    assert.equal(
      (await fetch(`${endpoint}/${randomUUID()}/report.zip`, { method: 'PUT', body: bytes }))
        .status,
      404,
    );
    const fallback = { ...metadata, id: randomUUID(), archiveExpected: false };
    assert.equal(
      (await fetch(endpoint, { method: 'POST', body: JSON.stringify(fallback) })).status,
      201,
    );
    assert.equal(
      JSON.parse(await readFile(join(root, fallback.id, 'metadata.json'), 'utf8')).archive.status,
      'unavailable',
    );
    assert.equal(simulatorReports().apply, 'serve');
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(root, { recursive: true });
  }
});
