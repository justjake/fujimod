import type { Completion } from './types.ts';
import type { ShareTrafficEvent } from './share-traffic.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { observeRequest } from './share-traffic.ts';
const request = () => ({
  id: 23,
  device: 'camera',
  timeout_ms: 5000,
  operation: { op: 'control_out', setup: { request: 0 }, data: Uint8Array.of(1, 2, 3) },
});
await test('each request and completion identify the controller, operation, bytes and result', async () => {
  const events: ShareTrafficEvent[] = [];
  let finish!: (completion: Completion) => void;
  const pending = observeRequest(
    'controller-2',
    request(),
    () =>
      new Promise<Completion>((resolve) => {
        finish = resolve;
      }),
    (event) => events.push(event),
  );
  assert.equal(events.length, 1);
  assert.equal(events[0].phase, 'submitted');
  assert.equal(events[0].controller, 'controller-2');
  assert.equal(events[0].id, 23);
  assert.deepEqual(events[0].operation.data, Uint8Array.of(1, 2, 3));
  assert.equal(events[0].timeoutMs, 5000);
  finish({ data: Uint8Array.of(4, 5), transferred: 2, error: null });
  assert.equal((await pending).transferred, 2);
  assert.equal(events.length, 2);
  assert.equal(events[1].phase, 'completed');
  assert.deepEqual('completion' in events[1] ? events[1].completion : undefined, {
    data: Uint8Array.of(4, 5),
    dataLength: 2,
    transferred: 2,
    error: null,
    deviceCount: 0,
  });
  assert.ok(events[1].phase === 'completed');
  assert.ok(events[1].elapsedMs >= 0);
});
await test('thrown ownership or execution errors still have a completion', async () => {
  const events: ShareTrafficEvent[] = [];
  const completion = await observeRequest(
    'other',
    request(),
    async () => {
      throw Error('Shared camera is in use');
    },
    (event) => events.push(event),
  );
  assert.equal(completion.error, 'Shared camera is in use');
  assert.equal(
    'completion' in events[1] ? events[1].completion.error : undefined,
    completion.error,
  );
});
await test('observer mutations and exceptions cannot modify or prevent the operation or reply', async () => {
  const frame = request();
  let ran = 0,
    observed = 0;
  const completion = await observeRequest(
    'controller',
    frame,
    async () => {
      ran++;
      assert.equal(frame.operation.op, 'control_out');
      assert.equal(frame.operation.setup.request, 0);
      return { data: Uint8Array.of(1, 2, 3), transferred: 3 };
    },
    (event) => {
      observed++;
      if (event.phase === 'submitted') Reflect.set(event.operation.setup!, 'request', 99);
      else Reflect.set(event.completion, 'data', 'changed');
    },
  );
  assert.equal(ran, 1);
  assert.equal(observed, 2);
  assert.deepEqual(completion.data, Uint8Array.of(1, 2, 3));
  assert.equal(completion.transferred, 3);
});
await test('diagnostics copy bounded prefixes and preserve full counts without exposing live binary buffers', async () => {
  const data = new Uint8Array(8 * 1024 * 1024).fill(255),
    events: ShareTrafficEvent[] = [];
  const frame = request();
  frame.operation.data = data;
  const completion = await observeRequest(
    'bulk',
    frame,
    async () => ({ data, transferred: data.length }),
    (event) => {
      events.push(event);
      const snapshot = event.phase === 'submitted' ? event.operation : event.completion;
      assert.equal(snapshot.data!.length, 4096);
      assert.equal(snapshot.dataLength, data.length);
      snapshot.data![0] = 0;
    },
  );
  assert.equal(events.length, 2);
  assert.equal(data[0], 255);
  assert.equal(completion.data, data);
});
