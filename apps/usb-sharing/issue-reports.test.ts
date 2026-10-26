import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runtime } from './test-runtime.ts';
const origin = 'https://justjake.github.io';
const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const tar = new Uint8Array(1024);
function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const b of bytes) {
    crc ^= b;
    for (let n = 0; n < 8; n++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return ((crc ^ 0xffffffff) >>> 0).toString(16).padStart(8, '0');
}
async function submission(overrides: Record<string, unknown> = {}) {
  const body = new FormData();
  body.set('report', new Blob([tar]), 'R1234567.TAR');
  body.set('description', 'Menu froze');
  body.set(
    'metadata',
    JSON.stringify({
      v: 1,
      firmware: 'x70-110',
      commit: 'a'.repeat(8),
      path: 'A:\\FUJIMOD\\REPORTS\\R1234567.TAR',
      bytes: 1024,
      crc32: crc32(tar),
      files: '403a0000',
      ...overrides,
    }),
  );
  const encoded = new Request('https://worker.example/issue-reports', { method: 'POST', body });
  const bytes = new Uint8Array(await encoded.arrayBuffer());
  return {
    method: 'POST',
    headers: {
      Origin: origin,
      'Idempotency-Key': id,
      'Content-Type': encoded.headers.get('Content-Type')!,
      'Content-Length': String(bytes.length),
    },
    body: bytes,
  };
}
await test('explicit intake saves privately, creates one issue and deduplicates retries', async () => {
  let issues = 0;
  const mf = await runtime({
    bindings: { GITHUB_ISSUES_TOKEN: 'test-only' },
    outboundService: async (request) => {
      assert.equal(request.url, 'https://api.github.com/repos/justjake/fujimod/issues');
      const body = (await request.json()) as { body: string };
      assert.match(body.body, /Menu froze/);
      assert.match(body.body, /stored privately/);
      assert.equal(body.body.includes('/report.tar'), false);
      issues++;
      return Response.json(
        { html_url: 'https://github.com/justjake/fujimod/issues/123' },
        { status: 201 },
      );
    },
  });
  try {
    const first = await mf.dispatchFetch(
      'https://worker.example/issue-reports',
      await submission(),
    );
    assert.equal(first.status, 201);
    assert.equal(((await first.json()) as { id: string }).id, id);
    assert.equal(
      (await mf.dispatchFetch('https://worker.example/issue-reports', await submission())).status,
      200,
    );
    assert.equal(issues, 1);
    const bucket = await mf.getR2Bucket('ISSUE_REPORTS');
    assert.deepEqual(
      new Uint8Array(await (await bucket.get(`${id}/report.tar`))!.arrayBuffer()),
      tar,
    );
    const rows = await (await mf.getD1Database('REPORTS_INDEX'))
      .prepare('SELECT key, kind, id, state, bytes, summary, metadata FROM objects ORDER BY key')
      .all<{ key: string; kind: string; id: string; state: string; bytes: number; summary: string; metadata: string }>();
    assert.deepEqual(
      rows.results.map(({ key, kind, state, summary }) => [key, kind, state, summary]),
      [
        [`${id}/metadata.json`, 'issue-report', 'stored', 'Menu froze'],
        [`${id}/report.tar`, 'issue-report', 'stored', 'Menu froze'],
      ],
    );
    assert.equal(rows.results[1].bytes, tar.length);
    assert.equal(JSON.parse(rows.results[1].metadata).firmware, 'x70-110');
    assert.equal(
      (await mf.dispatchFetch(`https://worker.example/issue-reports/${id}/report.tar`)).status,
      404,
    );
  } finally {
    await mf.dispose();
  }
});
await test('rejects unconfigured service, invalid data and untrusted origins without submitting', async () => {
  let calls = 0;
  const mf = await runtime({
    outboundService: async () => {
      calls++;
      throw Error('Must not submit');
    },
  });
  try {
    assert.equal(
      (await mf.dispatchFetch('https://worker.example/issue-reports', await submission())).status,
      503,
    );
    const request = await submission();
    request.headers.Origin = 'https://attacker.example';
    assert.equal(
      (await mf.dispatchFetch('https://worker.example/issue-reports', request)).status,
      403,
    );
    assert.equal(calls, 0);
  } finally {
    await mf.dispose();
  }
  const configured = await runtime({
    bindings: { GITHUB_ISSUES_TOKEN: 'test-only' },
    outboundService: async () => {
      calls++;
      throw Error('Must not submit');
    },
  });
  try {
    assert.equal(
      (
        await configured.dispatchFetch(
          'https://worker.example/issue-reports',
          await submission({ crc32: '00000000' }),
        )
      ).status,
      400,
    );
    assert.equal(calls, 0);
  } finally {
    await configured.dispose();
  }
});
await test('uncertain GitHub response retains the archive without duplicate issues', async () => {
  let calls = 0;
  const mf = await runtime({
    bindings: { GITHUB_ISSUES_TOKEN: 'test-only' },
    outboundService: async () => {
      calls++;
      return new Response('Unavailable', { status: 502 });
    },
  });
  try {
    const response = await mf.dispatchFetch(
      'https://worker.example/issue-reports',
      await submission(),
    );
    assert.equal(response.status, 202);
    assert.deepEqual(await response.json(), { id });
    await mf.dispatchFetch('https://worker.example/issue-reports', await submission());
    assert.equal(calls, 1);
    assert.ok(await (await mf.getR2Bucket('ISSUE_REPORTS')).get(`${id}/report.tar`));
  } finally {
    await mf.dispose();
  }
});
