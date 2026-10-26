import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { WasmEngine } from './wasm-engine';
import type {
  WasmWorkerEvent,
  WasmWorkerRequest,
} from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
import { testConfig } from './test-fixtures';

function setup() {
  Object.assign(globalThis, { document: { baseURI: 'https://example.invalid/' } });
  const sent: WasmWorkerRequest[] = [],
    errors: Error[] = [];
  const worker = {
    onmessage: (_event: { data: WasmWorkerEvent }) => {},
    onerror: (_event: { message: string }) => {},
    onmessageerror: () => {},
    terminated: false,
    postMessage(message: WasmWorkerRequest) {
      sent.push(message);
    },
    terminate() {
      this.terminated = true;
    },
  };
  const engine = new WasmEngine(
    (error) => errors.push(error),
    () => worker as unknown as Worker,
  );
  const reply = (event: WasmWorkerEvent) => worker.onmessage({ data: event });
  const resolve = (result?: unknown) => {
    const last = sent.at(-1)!;
    assert(last.type === 'rpc');
    reply({ type: 'reply', serial: last.serial, result });
  };
  return { engine, sent, worker, errors, reply, resolve };
}
test('main adapter transfers a DAT, selects absolute runtime paths and preserves all input arguments', async () => {
  const f = setup();
  const boot = f.engine.boot(new ArrayBuffer(2), testConfig(false), () => {});
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  const message = f.sent.at(-1)!;
  assert(message.type === 'rpc' && message.request.action === 'boot');
  assert.equal(message.request.config.bundle, 'https://example.invalid/runtime/');
  f.resolve([]);
  await boot;
  const press = f.engine.press('shutter', 'half');
  const input = f.sent.at(-1)!;
  assert(input.type === 'rpc');
  assert.deepEqual(input.request, {
    action: 'press',
    id: 'shutter',
    stage: 'half',
  });
  f.resolve();
  await press;
  for (const phase of ['press', 'drag', 'release', 'tap'] as const) {
    const task = f.engine.touch('touchscreen', phase, 100, 200);
    const message = f.sent.at(-1)!;
    assert(message.type === 'rpc');
    assert.deepEqual(message, {
      type: 'rpc',
      serial: message.serial,
      request: { action: 'touch', id: 'touchscreen', phase, x: 100, y: 200 },
    });
    f.resolve();
    await task;
  }
  const rotation = f.engine.rotate('ring', -2);
  f.resolve();
  await rotation;
  const read = f.engine.readCardFile('/a', { revision: 2, maxBytes: 200 });
  f.resolve(new Uint8Array([1, 2]));
  assert.deepEqual([...(await read)], [1, 2]);
  f.engine.dispose();
});
test('shared frame listeners receive the original heap view and release the worker lease even when rendering throws', () => {
  const f = setup(),
    memory = new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
  const buffer = memory.buffer;
  assert(buffer instanceof SharedArrayBuffer);
  new Uint8Array(memory.buffer).set([1, 2, 3, 255], 64);
  f.engine.frames((frame) => {
    assert('rgba' in frame);
    assert.equal(frame.rgba.buffer, memory.buffer);
    assert.deepEqual([...frame.rgba], [1, 2, 3, 255]);
    throw Error('Renderer failed');
  });
  assert.throws(
    () =>
      f.reply({
        type: 'frame_view',
        buffer,
        offset: 64,
        length: 4,
        width: 1,
        height: 1,
        sequence: 1,
        lease: 3,
      }),
    /Renderer failed/,
  );
  assert.deepEqual(f.sent.at(-1), { type: 'frame_release', lease: 3 });
  f.engine.dispose();
});
test('bitmap lifetime ends after synchronous presentation; worker failure and disposal settle pending requests', async () => {
  const f = setup();
  let closed = 0;
  const bitmap = {
    close() {
      closed++;
    },
  } as ImageBitmap;
  f.engine.frames((frame) => {
    assert('bitmap' in frame && frame.bitmap === bitmap);
    assert.equal(closed, 0);
  });
  f.reply({ type: 'frame', bitmap, width: 2, height: 2, sequence: 1 });
  assert.equal(closed, 1);
  const pending = f.engine.cardState();
  f.worker.onerror({ message: 'Worker stopped' });
  await assert.rejects(pending, /unexpectedly/);
  assert.equal(f.errors.length, 1);
  await assert.rejects(f.engine.profile(), /Worker stopped/);
  f.engine.dispose();
  assert(f.worker.terminated);
  await assert.rejects(f.engine.cardState(), /closed/);
});

test('a guest fault keeps the transport available for rollback and diagnostic export', async () => {
  const f = setup();
  f.reply({
    type: 'error',
    error: 'guest load',
    fault: { kind: 'guest-fault', message: 'Guest stopped', details: {} },
  });
  const rollback = f.engine.rollback(10);
  f.resolve({ power: 'on' });
  await rollback;
  const pending = f.engine.profile();
  f.engine.dispose();
  await assert.rejects(pending, /closed/);
});

test('Wasm download events preserve per-asset progress at the engine boundary', async () => {
  const f = setup();
  const updates: unknown[] = [];
  const boot = f.engine.boot(new ArrayBuffer(2), testConfig(false), (...args) =>
    updates.push(args),
  );
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  const assets = [
    {
      url: '/runtime/camera.wasm',
      label: 'Camera emulator',
      loaded: 1024,
      total: 4096,
      status: 'loading' as const,
    },
  ];
  f.reply({ type: 'progress', message: 'Camera emulator', percent: 25, assets });
  assert.deepEqual(updates.at(-1), ['Camera emulator', 25, assets]);
  f.resolve([]);
  await boot;
  f.engine.dispose();
});

test('portable snapshot loading resolves a relative runtime bundle before dispatch', async () => {
  const f = setup();
  const loading = f.engine.loadSnapshot(new Uint8Array([1, 2, 3]), testConfig(false), () => {});
  const message = f.sent.at(-1)!;
  assert(message.type === 'rpc' && message.request.action === 'load_snapshot');
  assert.equal(message.request.config.bundle, 'https://example.invalid/runtime/');
  assert.deepEqual([...message.request.bytes], [1, 2, 3]);
  f.resolve([]);
  await loading;
  const exported = f.engine.exportSnapshot();
  f.resolve(new Uint8Array([4, 5]));
  assert.deepEqual([...(await exported)], [4, 5]);
  f.engine.dispose();
});

test('the X70 boots and restores its default with original RAF bytes', async () => {
  const f = setup();
  const bytes = new Uint8Array([82, 65, 70]);
  const gzip = await new Response(
    new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip')),
  ).arrayBuffer();
  const original = globalThis.fetch;
  const urls: string[] = [];
  globalThis.fetch = async (input) => {
    urls.push(input instanceof Request ? input.url : input.toString());
    return new Response(gzip);
  };
  const sent = async (action: string) => {
    for (let wait = 0; wait < 500; wait++) {
      const last = f.sent.at(-1);
      if (last?.type === 'rpc' && last.request.action === action) return last;
      await new Promise<void>((resolve) => setTimeout(resolve, 1));
    }
    throw Error(`No ${action} request`);
  };
  try {
    const boot = f.engine.boot(
      new ArrayBuffer(1),
      { ...testConfig(), firmwareId: 'x70-110' },
      () => {},
    );
    const request = await sent('boot');
    assert(request.type === 'rpc' && request.request.action === 'boot');
    assert.deepEqual(request.request.scene, { raf: bytes });
    assert.match(urls[0]!, /build\/assets\/default-scene\.raf\.gzipdata/);
    f.resolve([]);
    await boot;
    const restore = f.engine.setScene(null);
    const message = await sent('scene');
    assert(message.type === 'rpc');
    assert.deepEqual(message.request, { action: 'scene', scene: { raf: bytes } });
    f.resolve();
    await restore;
    assert.equal(urls.length, 2);
  } finally {
    globalThis.fetch = original;
    f.engine.dispose();
  }
});
