import type { Message, ResultMessage, Ticket, Operation } from './types.ts';
import { exporterFixture } from './exporter-fixture.ts';
import type { ShareTrafficEvent } from './share-traffic.ts';
// Exercises ShareHost with isolated signaling and data channels. No network or USB access.
import test from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { VERSION, encodeMessage, decodeMessage } from './wire.ts';
import { ShareHost, newTicket } from './peer.ts';
class Socket {
  onopen?: () => void;
  onmessage?: (event: { data: string }) => void;
  onclose?: () => void;
  constructor() {
    queueMicrotask(() => this.onopen?.());
  }
  send(text: string) {
    if (JSON.parse(text).type === 'join')
      queueMicrotask(() => this.onmessage?.({ data: '{"type":"ready"}' }));
  }
  close() {
    this.onclose?.();
  }
}
class Connection extends EventTarget {
  static all: Connection[] = [];
  iceGatheringState: string;
  connectionState: string;
  localDescription?: { type: string; sdp: string; toJSON(): object };
  ondatachannel?: (event: { channel: Channel }) => void;
  constructor() {
    super();
    this.iceGatheringState = 'complete';
    this.connectionState = 'connected';
    Connection.all.push(this);
  }
  async setRemoteDescription() {}
  async createAnswer() {
    return { type: 'answer', sdp: 'v=0' };
  }
  async setLocalDescription(answer: { type: string; sdp: string }) {
    this.localDescription = { ...answer, toJSON: () => answer };
  }
  close() {}
}
class Channel extends EventTarget {
  readyState = 'open';
  bufferedAmount = 0;
  sent: Message[] = [];
  failReplies = false;
  onclose?: () => void;
  onmessage?: (event: { data: ArrayBuffer }) => void;
  send(packet: Uint8Array) {
    const frame = decodeMessage(packet.subarray(1));
    if (this.failReplies && frame.type === 'result') throw Error('Data channel send failed');
    this.sent.push(frame);
  }
  close() {
    if (this.readyState === 'closed') return;
    this.readyState = 'closed';
    this.onclose?.();
    this.dispatchEvent(new Event('close'));
  }
  input(frame: Message) {
    const bytes = encodeMessage(frame),
      packet = new Uint8Array(bytes.length + 1);
    packet[0] = 1;
    packet.set(bytes, 1);
    this.onmessage?.({ data: packet.buffer });
  }
}
async function until(predicate: () => boolean) {
  for (let i = 0; i < 200; i++) {
    if (predicate()) return;
    await delay(1);
  }
  assert.ok(predicate(), 'ShareHost did not complete');
}
async function offer(ticket: Ticket, id: string) {
  const iv = new Uint8Array(12);
  const key = await crypto.subtle.importKey(
    'raw',
    Buffer.from(ticket.key, 'base64url'),
    'AES-GCM',
    false,
    ['encrypt'],
  );
  const encrypted = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: new TextEncoder().encode(ticket.room + '/' + id) },
    key,
    new TextEncoder().encode('{"type":"offer","sdp":"v=0"}'),
  );
  return Buffer.concat([iv, new Uint8Array(encrypted)]).toString('base64url');
}
async function fixture(observer?: (event: ShareTrafficEvent) => void) {
  const oldSocket = globalThis.WebSocket,
    oldConnection = globalThis.RTCPeerConnection;
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  globalThis.RTCPeerConnection = Connection as unknown as typeof RTCPeerConnection;
  const calls: string[] = [],
    errors: Error[] = [],
    events: ShareTrafficEvent[] = [],
    ticket = newTicket('wss://isolated.test/signal', []);
  const device = exporterFixture(
    {
      id: 'camera',
      vendor_id: 0,
      product_id: 0,
      manufacturer: null,
      product: null,
      serial: null,
      interfaces: [],
    },
    async (op) => {
      calls.push(op.op);
      return op.op === 'transfer_in'
        ? { data: Uint8Array.of(1, 2, 3), transferred: 3 }
        : {
            transferred:
              op.op === 'transfer_out' || op.op === 'control_out' ? (op.data?.length ?? 0) : 12,
          };
    },
  );
  const host = await ShareHost.create(ticket, device, {
    traffic: (event) => {
      events.push(event);
      observer?.(event);
    },
    error: (error) => errors.push(error),
  });
  await host.message({
    type: 'signal',
    peer: 'controller',
    from: 'controller',
    data: await offer(ticket, 'controller'),
  });
  const connection = Connection.all.at(-1)!,
    channel = new Channel();
  connection.ondatachannel!({ channel });
  channel.input({
    type: 'hello',
    role: 'controller',
    version: VERSION,
    token: ticket.key,
    reason: 'isolated test',
  });
  await until(() => channel.sent.some((frame) => frame.type === 'ready'));
  return {
    channel,
    events,
    calls,
    errors,
    async request(seq: number, op: Operation) {
      channel.input({ type: 'execute', seq, device: 'camera', op, timeout_ms: 5000 });
      await until(() => channel.sent.some((frame) => 'seq' in frame && frame.seq === seq));
      const reply = channel.sent.find(
        (frame): frame is ResultMessage => frame.type === 'result' && frame.seq === seq,
      );
      assert.ok(reply);
      return reply.completion;
    },
    dispose() {
      host.stop();
      globalThis.WebSocket = oldSocket;
      globalThis.RTCPeerConnection = oldConnection;
    },
  };
}
await test('ShareHost reports every request/completion, including ownership failures', async () => {
  const f = await fixture();
  try {
    await f.request(1, { op: 'list' });
    await f.request(2, { op: 'probe' });
    await f.request(3, { op: 'open' });
    assert.deepEqual(
      (await f.request(4, { op: 'transfer_in', endpoint: 0x82, length: 64 })).data,
      Uint8Array.of(1, 2, 3),
    );
    await f.request(5, { op: 'close' });
    const failed = await f.request(6, { op: 'transfer_in', endpoint: 0x82, length: 64 });
    assert.match(failed.error!, /not claimed/);
    assert.deepEqual(f.calls, ['open', 'transfer_in', 'close']);
    assert.equal(f.events.length, 12);
    for (let i = 0; i < 6; i++) {
      assert.equal(f.events[i * 2].phase, 'submitted');
      assert.equal(f.events[i * 2 + 1].phase, 'completed');
      assert.equal(f.events[i * 2].id, i + 1);
      assert.equal(f.events[i * 2].controller, 'controller');
    }
    const last = f.events.at(-1)!;
    assert.ok(last.phase === 'completed');
    assert.match(last.completion.error!, /not claimed/);
  } finally {
    f.dispose();
  }
});
await test('ShareHost still replies when observers throw and reports completion before a send fails', async () => {
  const f = await fixture(() => {
    throw Error('Observer failed');
  });
  try {
    await f.request(1, { op: 'open' });
    assert.equal(f.errors.length, 0);
    f.channel.failReplies = true;
    f.channel.input({
      type: 'execute',
      seq: 2,
      device: 'camera',
      op: { op: 'transfer_in', endpoint: 0x82, length: 64 },
      timeout_ms: 5000,
    });
    await until(() => f.channel.readyState === 'closed');
    const last = f.events.at(-1)!;
    assert.ok(last.phase === 'completed');
    assert.equal(last.completion.transferred, 3);
    assert.match(f.errors[0].message, /send failed/);
  } finally {
    f.dispose();
  }
});
await test('ShareHost passes notices through in pipeline order and answers a pipelined sync', async () => {
  const f = await fixture();
  try {
    await f.request(1, { op: 'open' });
    f.channel.input({ type: 'activity', title: 'Uploading' });
    f.channel.input({ type: 'progress_start', label: 'Uploading', total: 93056 });
    f.channel.input({
      type: 'execute',
      seq: 2,
      device: 'camera',
      timeout_ms: 5000,
      op: { op: 'transfer_out', endpoint: 1, data: Uint8Array.of(1, 2) },
    });
    f.channel.input({
      type: 'execute',
      seq: 3,
      device: 'camera',
      timeout_ms: 5000,
      op: { op: 'transfer_in', endpoint: 0x82, length: 64 },
    });
    f.channel.input({ type: 'expect', seq: 4, of: 3, offset: 1, bytes: Uint8Array.of(2, 3) });
    f.channel.input({ type: 'progress_update', done: 16384 });
    f.channel.input({ type: 'sync', seq: 5 });
    f.channel.input({ type: 'progress_done' });
    await until(() => f.channel.sent.some((m) => m.type === 'synced'));
    assert.deepEqual(
      f.channel.sent.filter((m) => m.type !== 'ready').map((m) => [m.type, 'seq' in m ? m.seq : 0]),
      [
        ['result', 1],
        ['result', 2],
        ['result', 3],
        ['synced', 5],
      ],
    );
    assert.deepEqual(f.calls, ['open', 'transfer_out', 'transfer_in']);
    assert.equal(f.channel.readyState, 'open');
    assert.equal(f.errors.length, 0);
  } finally {
    f.dispose();
  }
});
