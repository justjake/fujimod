import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runtime } from './test-runtime.ts';
const origin = 'https://justjake.github.io';
const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const files = [{ model: 'X/70', serial: '59 01', name: 'flash.bin', bytes: 100 }];
const MiB = 1024 * 1024;
await test('real R2 multipart upload resumes, finishes privately, and deduplicates calls', async () => {
  const mf = await runtime({
    outboundService: async () => {
      throw Error('No outbound calls');
    },
  });
  const call = (path: string, method = 'POST', body: object | Uint8Array = {}) =>
    mf.dispatchFetch(`https://worker.example/contributions/${id}${path}`, {
      method,
      headers: { Origin: origin },
      ...(method === 'GET'
        ? {}
        : { body: body instanceof Uint8Array ? body : JSON.stringify(body) }),
    });
  try {
    assert.equal((await call('', 'POST', { files })).status, 201);
    assert.equal((await call('', 'POST', { files })).status, 200);
    assert.equal((await call('', 'POST', { files: [{ ...files[0], bytes: 101 }] })).status, 409);
    assert.equal((await call('/finish', 'POST', { manifest: {}, message: '' })).status, 409);
    const started = (await (await call('/files/0')).json()) as { uploadId: string; key: string };
    assert.equal(started.key, `models/X_70/cameras/59_01/${id}/flash.bin.gz`);
    const index = await mf.getD1Database('REPORTS_INDEX');
    const row = (key: string) =>
      index
        .prepare('SELECT kind, id, state, bytes, stored_at FROM objects WHERE key = ?1')
        .bind(key)
        .first<{ kind: string; id: string; state: string; bytes: number | null; stored_at: string | null }>();
    assert.deepEqual(await row(started.key), {
      kind: 'contribution',
      id,
      state: 'uploading',
      bytes: null,
      stored_at: null,
    });
    assert.equal(
      ((await (await call('/files/0')).json()) as { uploadId: string }).uploadId,
      started.uploadId,
    );
    const first = new Uint8Array(5 * MiB).fill(42),
      last = new Uint8Array([1, 2, 3]);
    assert.equal((await call('/files/0/parts/1', 'PUT', first)).status, 200);
    const status = (await (await call('', 'GET')).json()) as {
      files: { parts: { partNumber: number; bytes: number }[] }[];
    };
    assert.equal(status.files[0].parts[0].bytes, first.length);
    assert.equal((await call('/files/0/parts/1', 'PUT', first)).status, 200);
    assert.equal((await call('/files/0/parts/2', 'PUT', last)).status, 200);
    assert.equal((await call('/files/0/complete', 'POST', { parts: 3 })).status, 400);
    assert.equal((await call('/files/0/complete', 'POST', { parts: 2 })).status, 200);
    assert.equal((await call('/files/0/complete', 'POST', { parts: 2 })).status, 200);
    const bucket = await mf.getR2Bucket('ISSUE_REPORTS');
    const object = await bucket.get(started.key);
    assert.equal(object!.size, first.length + last.length);
    const stored = await row(started.key);
    assert.equal(stored?.state, 'stored');
    assert.equal(stored?.bytes, first.length + last.length);
    assert.ok(stored?.stored_at);
    assert.deepEqual(new Uint8Array(await object!.arrayBuffer()).subarray(first.length), last);
    const finish = { manifest: { files: ['flash.bin'] }, message: 'Camera capture' };
    assert.equal((await call('/finish', 'POST', finish)).status, 200);
    assert.equal((await call('/finish', 'POST', { manifest: {}, message: 'changed' })).status, 200);
    assert.deepEqual(
      JSON.parse(await (await bucket.get(`contributions/${id}/manifest.json`))!.text()),
      finish.manifest,
    );
    assert.deepEqual(
      JSON.parse(await (await bucket.get(`contributions/${id}/message.json`))!.text()),
      { message: finish.message },
    );
    assert.equal(((await (await call('', 'GET')).json()) as { finished: boolean }).finished, true);
    assert.equal((await mf.dispatchFetch(`https://worker.example/${started.key}`)).status, 404);
  } finally {
    await mf.dispose();
  }
});
await test('origin, metadata, multipart sequence and actual request sizes are enforced', async () => {
  const mf = await runtime();
  const base = `https://worker.example/contributions/${id}`;
  const send = (path: string, body: object | Uint8Array, method = 'POST') =>
    mf.dispatchFetch(base + path, {
      method,
      headers: { Origin: origin },
      body: body instanceof Uint8Array ? body : JSON.stringify(body),
    });
  try {
    assert.equal(
      (
        await mf.dispatchFetch(base, {
          method: 'POST',
          headers: { Origin: 'https://other.example' },
        })
      ).status,
      403,
    );
    const preflight = await mf.dispatchFetch(base, {
      method: 'OPTIONS',
      headers: { Origin: origin },
    });
    assert.equal(preflight.status, 204);
    assert.equal(preflight.headers.get('Access-Control-Allow-Origin'), origin);
    assert.match(preflight.headers.get('Access-Control-Allow-Methods')!, /PUT/);
    assert.equal(
      (await send('', { files: [files[0], { ...files[0], model: 'X_70' }] })).status,
      400,
    );
    assert.equal((await send('', { files: [{ ...files[0], name: '..' }] })).status, 400);
    assert.equal((await send('', { files })).status, 201);
    assert.equal((await send('/files/0', {})).status, 200);
    assert.equal((await send('/files/0/parts/1', new Uint8Array(1), 'PUT')).status, 200);
    assert.equal((await send('/files/0/parts/2', new Uint8Array(1), 'PUT')).status, 200);
    assert.equal((await send('/files/0/complete', { parts: 2 })).status, 400);
    assert.equal((await send('/files/0/parts/0', new Uint8Array(1), 'PUT')).status, 400);
    const streamedPart = (bytes: number) => {
      let remaining = bytes;
      return mf.dispatchFetch(base + '/files/0/parts/3', {
        method: 'PUT',
        headers: { Origin: origin },
        duplex: 'half',
        body: new ReadableStream({
          pull(controller) {
            if (!remaining) {
              controller.close();
              return;
            }
            const size = Math.min(remaining, MiB);
            remaining -= size;
            controller.enqueue(new Uint8Array(size));
          },
        }),
      });
    };
    assert.equal((await streamedPart(64 * MiB + 1)).status, 413);
    assert.equal((await send('/files/0/parts/3', new Uint8Array(), 'PUT')).status, 400);
    assert.equal((await streamedPart(64 * MiB)).status, 200);
  } finally {
    await mf.dispose();
  }
});
await test('daily contribution budget is separate and retries do not consume slots', async () => {
  const mf = await runtime();
  try {
    for (let n = 0; n < 51; n++) {
      const contribution = `aaaaaaaa-aaaa-4aaa-8aaa-${n.toString(16).padStart(12, '0')}`;
      const response = await mf.dispatchFetch(
        `https://worker.example/contributions/${contribution}`,
        {
          method: 'POST',
          headers: { Origin: origin, 'CF-Connecting-IP': `192.0.2.${n}` },
          body: JSON.stringify({ files }),
        },
      );
      assert.equal(response.status, n < 50 ? 201 : 429);
    }
    const retry = await mf.dispatchFetch(
      'https://worker.example/contributions/aaaaaaaa-aaaa-4aaa-8aaa-000000000000',
      {
        method: 'POST',
        headers: { Origin: origin },
        body: JSON.stringify({ files }),
      },
    );
    assert.equal(retry.status, 200);
    const limited = await Promise.all(
      Array.from({ length: 6 }, () =>
        mf.dispatchFetch(`https://worker.example/contributions/${id}`, {
          method: 'POST',
          headers: { Origin: origin, 'CF-Connecting-IP': '192.0.2.200' },
          body: JSON.stringify({ files }),
        }),
      ),
    );
    assert.ok(
      limited.some(
        (response) =>
          response.status === 429 && response.headers.get('Access-Control-Allow-Origin') === origin,
      ),
    );
  } finally {
    await mf.dispose();
  }
});
await test('R2 completion and finish recover after an interrupted D1 index write', async () => {
  const mf = await runtime();
  const base = `https://worker.example/contributions/${id}`;
  const send = (path: string, body: object | Uint8Array = {}, method = 'POST') =>
    mf.dispatchFetch(base + path, {
      method,
      headers: { Origin: origin },
      body: body instanceof Uint8Array ? body : JSON.stringify(body),
    });
  try {
    await send('', { files });
    const file = (await (await send('/files/0')).json()) as { key: string };
    await send('/files/0/parts/1', new Uint8Array([1, 2, 3]), 'PUT');
    const index = await mf.getD1Database('REPORTS_INDEX');
    await index.prepare('ALTER TABLE objects RENAME TO interrupted_objects').run();
    assert.equal((await send('/files/0/complete', { parts: 1 })).status, 503);
    assert.ok(await (await mf.getR2Bucket('ISSUE_REPORTS')).head(file.key));
    await index.prepare('ALTER TABLE interrupted_objects RENAME TO objects').run();
    assert.equal((await send('/files/0/complete', { parts: 1 })).status, 200);
    assert.equal(
      (await index.prepare('SELECT kind FROM objects WHERE key = ?').bind(file.key).first())?.kind,
      'contribution',
    );
    await index.prepare('ALTER TABLE objects RENAME TO interrupted_objects').run();
    assert.equal(
      (await send('/finish', { manifest: { original: true }, message: 'First message' })).status,
      503,
    );
    await index.prepare('ALTER TABLE interrupted_objects RENAME TO objects').run();
    assert.equal(
      (await send('/finish', { manifest: { changed: true }, message: 'Changed message' })).status,
      200,
    );
    const bucket = await mf.getR2Bucket('ISSUE_REPORTS');
    assert.deepEqual(
      JSON.parse(await (await bucket.get(`contributions/${id}/manifest.json`))!.text()),
      { original: true },
    );
    assert.deepEqual(
      JSON.parse(await (await bucket.get(`contributions/${id}/message.json`))!.text()),
      { message: 'First message' },
    );
    assert.equal(
      (
        await index
          .prepare('SELECT count(*) AS count FROM objects WHERE kind = ? AND id = ?')
          .bind('contribution', id)
          .first()
      )?.count,
      3,
    );
  } finally {
    await mf.dispose();
  }
});
