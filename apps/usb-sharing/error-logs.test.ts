import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runtime } from './test-runtime.ts';
import { clientErrorLog, type ClientErrorRecord } from './client-error-log.ts';
const origin = 'https://justjake.github.io';
const url = 'https://worker.example/error-logs';
const record: ClientErrorRecord = {
  id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  session: '3f2a9c10-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  kind: 'error',
  message: "TypeError: null is not an object (evaluating 'node.nextSibling')",
  stack: 'hydrate@https://fujimod.com/assets/web.js:1:2',
  page: 'https://fujimod.com/dev-tools/',
  build: 'abc123 / 2026-10-24',
  userAgent: 'Mozilla/5.0 (iPhone)',
  device: {
    browser: 'Safari 27.0.1',
    os: 'iOS',
    screen: { width: 390, height: 844, pixelRatio: 3 },
    viewport: { width: 390, height: 664 },
  },
  createdAt: '2026-10-24T14:20:00.000Z',
  uptime: 1234,
};

test('a log entry names the browser session, page and error, separate from the Worker', () => {
  const entry = clientErrorLog(record, 'US');
  assert.equal(
    entry.message,
    "[browser 3f2a9c10 · Safari 27.0.1 · iOS 390×844] error on /dev-tools/: TypeError: null is not an object (evaluating 'node.nextSibling')",
  );
  assert.equal(entry.source, 'website-client');
  assert.equal(entry.client.session, record.session);
  assert.equal(entry.client.country, 'US');
  assert.deepEqual(entry.client.device, record.device);
  assert.equal(entry.client.secondsSinceLoad, 1.2);
  assert.equal(entry.error.stack, record.stack);
});

await test('intake accepts valid records from allowed origins only', async () => {
  const mf = await runtime();
  try {
    const post = (body: string, from = origin) =>
      mf.dispatchFetch(url, {
        method: 'POST',
        headers: { Origin: from, 'Content-Type': 'text/plain' },
        body,
      });
    const saved = await post(JSON.stringify(record));
    assert.equal(saved.status, 201);
    assert.equal(saved.headers.get('Access-Control-Allow-Origin'), origin);
    assert.equal((await post(JSON.stringify(record), 'https://evil.example')).status, 403);
    assert.equal((await post('not json')).status, 400);
    assert.equal((await post(JSON.stringify({ ...record, kind: 'other' }))).status, 400);
    assert.equal((await post(JSON.stringify({ ...record, device: 'iPhone' }))).status, 400);
    assert.equal(
      (await post(JSON.stringify({ ...record, message: 'x'.repeat(20000) }))).status,
      400,
    );
    assert.equal((await mf.dispatchFetch(url, { headers: { Origin: origin } })).status, 405);
  } finally {
    await mf.dispose();
  }
});
