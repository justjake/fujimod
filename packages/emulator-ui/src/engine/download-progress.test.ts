import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { DownloadProgress } from './download-progress';

test('reports actual streamed bytes, percentage and estimated remaining time without buffering', async () => {
  let time = 0;
  const updates: [string, number | undefined][] = [];
  const tracker = new DownloadProgress(
    (message, percent) => updates.push([message, percent]),
    () => time,
  );
  let feed!: ReadableStreamDefaultController<Uint8Array>;
  const original = new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        feed = controller;
      },
    }),
    {
      headers: { 'Content-Length': String(4 * 1048576), 'Content-Type': 'application/wasm' },
    },
  );
  const response = tracker.response('https://test.invalid/camera.wasm', original);
  assert.equal(response.headers.get('content-type'), 'application/wasm');
  assert.equal(updates.at(-1)?.[1], 0);
  const reader = response.body!.getReader();
  time = 2000;
  feed.enqueue(new Uint8Array(1048576).fill(23));
  assert.equal((await reader.read()).value?.[0], 23);
  assert.deepEqual(updates.at(-1), [
    'Camera emulator · 1.0 / 4.0 MiB · 0.5 MiB/s · ~6 s remaining',
    25,
  ]);
  time = 4000;
  feed.enqueue(new Uint8Array(3 * 1048576).fill(42));
  assert.equal((await reader.read()).value?.at(-1), 42);
  assert.equal(updates.at(-1)?.[1], 100);
  feed.close();
  assert.equal((await reader.read()).done, true);
  assert.deepEqual(updates.at(-1), ['Initializing camera runtime…', undefined]);
});

test('unknown and compressed wire lengths do not fabricate percentages or ETAs', async () => {
  for (const headers of [
    new Headers(),
    new Headers({ 'Content-Length': '5', 'Content-Encoding': 'gzip' }),
  ]) {
    const updates: [string, number | undefined][] = [];
    let time = 0;
    const tracker = new DownloadProgress(
      (message, percent) => updates.push([message, percent]),
      () => time,
    );
    const response = tracker.response(
      'https://test.invalid/camera.mjs',
      new Response('decoded bytes', { headers }),
    );
    time = 2000;
    assert.equal(await response.text(), 'decoded bytes');
    assert.ok(
      updates.every(
        ([message, percent]) => percent === undefined && !message.includes('remaining'),
      ),
    );
    assert.ok(updates.some(([message]) => message.includes('MiB/s')));
  }
});

test('compressed assets use an explicit decoded length, and cached bodies retain byte progress', async () => {
  const updates: number[] = [];
  const tracker = new DownloadProgress((_message, percent) => {
    if (percent !== undefined) updates.push(percent);
  });
  const response = tracker.response(
    'https://test.invalid/camera.wasm',
    new Response('decoded bytes', {
      headers: {
        'Content-Length': '5',
        'Content-Encoding': 'gzip',
        'X-Decoded-Content-Length': '13',
      },
    }),
  );
  assert.equal(await response.text(), 'decoded bytes');
  assert.equal(updates[0], 0);
  assert.ok(updates.includes(100));
});

test('parallel downloads keep one asset selected and move to the next at completion', async () => {
  const updates: [string, number | undefined][] = [];
  const tracker = new DownloadProgress((message, percent) => updates.push([message, percent]));
  const first = tracker.response(
    'https://test.invalid/camera.mjs',
    new Response('abc', { headers: { 'Content-Length': '3' } }),
  );
  const second = tracker.response(
    'https://test.invalid/camera.wasm',
    new Response('def', { headers: { 'Content-Length': '3' } }),
  );
  assert.ok(updates.every(([message]) => message.startsWith('Runtime loader')));
  await first.text();
  assert.ok(updates.at(-1)?.[0].startsWith('Camera emulator'));
  await second.text();
  assert.equal(updates.at(-1)?.[0], 'Initializing camera runtime…');
});

test('stream failures propagate to the runtime consumer', async () => {
  let assets: import('./download-progress').AssetProgress[] = [];
  const tracker = new DownloadProgress((_message, _percent, value) => {
    assets = value ?? [];
  });
  const response = tracker.response(
    'https://test.invalid/camera.mjs',
    new Response(
      new ReadableStream({
        start(controller) {
          controller.error(Error('Transfer interrupted'));
        },
      }),
    ),
  );
  await assert.rejects(response.arrayBuffer(), /Transfer interrupted/);
  assert.equal(assets[0].status, 'failed');
});

test('reports waiting assets before headers and retains parallel assets after they finish', async () => {
  let latest: import('./download-progress').AssetProgress[] = [];
  let respond!: (response: Response) => void;
  const tracker = new DownloadProgress((_message, _percent, assets) => {
    latest = assets ?? [];
  });
  const request = tracker.fetch(
    'https://test.invalid/camera.mjs',
    () =>
      new Promise((resolve) => {
        respond = resolve;
      }),
  );
  assert.equal(latest[0].status, 'waiting');
  const second = tracker.response('https://test.invalid/camera.wasm', new Response('abc'));
  await second.text();
  assert.equal(latest.length, 2);
  assert.equal(latest[0].status, 'waiting');
  assert.equal(latest[1].status, 'complete');
  respond(new Response('checked bytes', { headers: { 'Content-Length': '13' } }));
  assert.equal(await (await request).text(), 'checked bytes');
  assert.equal(latest[0].status, 'complete');
  assert.equal(latest[0].loaded, 13);
  assert.equal(latest[1].status, 'complete');
});
