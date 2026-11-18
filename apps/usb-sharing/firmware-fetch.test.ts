import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runtime } from './test-runtime.ts';
import { firmwareSource } from './firmware-source.ts';

const origin = 'https://fujimod.com';
const source = 'https://dl.fujifilm-x.com/support/firmware/x70mivua0pf/FPUPDATE.DAT';
const endpoint = (url = source) =>
  `https://worker.example/firmware-fetch?url=${encodeURIComponent(url)}`;

await test('source policy accepts official paths and rejects host and path tricks', () => {
  for (const url of [
    source,
    'https://fujifilm-korea.co.kr/support/download/attachment/8250/0',
    'https://www.fujifilm-korea.co.kr/support/download/attachment/11311/0',
  ])
    assert.equal(firmwareSource(url)?.href, url);
  for (const url of [
    '',
    'not a URL',
    source.replace('https:', 'http:'),
    source.replace('.com/', '.com.evil.example/'),
    source.replace('dl.', 'user@dl.'),
    source.replace('.com/', '.com:8443/'),
    source.replace('/support/firmware/', '/other/'),
    source.replace('FPUPDATE.DAT', '%2e%2e/FPUPDATE.DAT'),
    source.replace('FPUPDATE.DAT', 'redirect.php'),
    `${source}#fragment`,
    'https://fujifilm-korea.co.kr/support/download/attachment/8250/0/extra',
  ])
    assert.equal(firmwareSource(url), undefined, url);
});

await test('Worker transfers bytes without forwarding browser credentials or upstream cookies', async () => {
  const bytes = new Uint8Array([0, 4, 255, 16]);
  const mf = await runtime({
    bindings: { ALLOWED_ORIGINS: origin },
    outboundService: async (request) => {
      assert.equal(request.url, source);
      assert.equal(request.method, 'GET');
      assert.equal(request.headers.get('Cookie'), null);
      assert.equal(request.headers.get('Authorization'), null);
      assert.equal(request.headers.get('Range'), null);
      return new Response(bytes, {
        headers: {
          'Content-Type': 'application/octet-stream',
          'Cache-Control': 'public, max-age=86400',
          'Set-Cookie': 'upstream=secret',
        },
      });
    },
  });
  try {
    const response = await mf.dispatchFetch(endpoint(), {
      headers: {
        Origin: origin,
        Cookie: 'browser=secret',
        Authorization: 'Bearer secret',
        Range: 'bytes=0-1',
      },
    });
    assert.equal(response.status, 200);
    assert.equal(response.headers.get('Access-Control-Allow-Origin'), origin);
    assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.equal(response.headers.get('Content-Type'), 'application/octet-stream');
    assert.equal(response.headers.get('Set-Cookie'), null);
    assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes);
  } finally {
    await mf.dispose();
  }
});

await test('Worker rejects invalid requests and never follows upstream redirects', async () => {
  let calls = 0;
  const mf = await runtime({
    bindings: { ALLOWED_ORIGINS: origin },
    outboundService: async () => {
      calls++;
      return Response.redirect('https://untrusted.example/firmware', 302);
    },
  });
  try {
    assert.equal((await mf.dispatchFetch(endpoint())).status, 403);
    assert.equal(
      (await mf.dispatchFetch(endpoint(), { headers: { Origin: 'https://evil.example' } })).status,
      403,
    );
    assert.equal(
      (await mf.dispatchFetch(endpoint(), { method: 'POST', headers: { Origin: origin } })).status,
      405,
    );
    assert.equal(
      (
        await mf.dispatchFetch(endpoint('https://evil.example/file'), {
          headers: { Origin: origin },
        })
      ).status,
      400,
    );
    assert.equal(calls, 0);
    const redirected = await mf.dispatchFetch(endpoint(), { headers: { Origin: origin } });
    assert.equal(redirected.status, 502);
    assert.equal(redirected.headers.get('Access-Control-Allow-Origin'), origin);
    assert.equal(calls, 1);
  } finally {
    await mf.dispose();
  }
});
