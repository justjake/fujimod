import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { VERSION, MAX_TRANSFER, encodeMessage, decodeMessage, expectationHolds } from './wire.ts';
import { PeerChannel } from './peer.ts';
import type { Message, ExecuteMessage } from './types.ts';
const vectors: Record<string, string> = JSON.parse(
  readFileSync(new URL('./vectors.json', import.meta.url), 'utf8'),
);
const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString('hex');
const setup = { request_type: 'vendor', recipient: 'other', request: 0, value: 0x1234, index: 1 };
const device = {
  id: '1',
  vendor_id: 0x04cb,
  product_id: 0xff80,
  manufacturer: 'FUJIFILM',
  product: null,
  serial: null,
  interfaces: [
    {
      configuration: 1,
      interface: 0,
      alternate: 0,
      endpoints: [{ address: 0x81, kind: 'bulk', packet_size: 512 }],
    },
  ],
  status: { online: true, battery: 'full', battery_age_ms: 300 },
};
const execute = (seq: number, op: ExecuteMessage['op']): Message => ({
  type: 'execute',
  seq,
  device: 'camera',
  timeout_ms: 5000,
  op,
});
const empty = new Uint8Array();
// The same values as `vectors()` in src/protocol.rs.
const messages: Record<string, Message> = {
  hello: { type: 'hello', version: VERSION, role: 'controller', token: 't', reason: 'install' },
  hello_without_reason: {
    type: 'hello',
    version: VERSION,
    role: 'exporter',
    token: 't',
    reason: null,
  },
  ready: { type: 'ready', version: VERSION, id: '7' },
  goodbye: { type: 'goodbye', reason: 'done' },
  status: { type: 'status', battery: 'full' },
  attention: { type: 'attention', message: 'Camera stopped answering' },
  activity: { type: 'activity', title: 'Uploading' },
  progress_start: { type: 'progress_start', label: 'Uploading', total: 93056 },
  progress_update: { type: 'progress_update', done: 16384 },
  progress_done: { type: 'progress_done' },
  inventory: { type: 'inventory', devices: [device] },
  probe: execute(1, { op: 'probe' }),
  list: execute(2, { op: 'list' }),
  owner_status: execute(3, { op: 'owner_status', message: 'stopped' }),
  owner_attention: execute(4, { op: 'owner_attention', message: 'Film it' }),
  open: execute(5, { op: 'open', configuration: 1, interface: 0, alternate: 0 }),
  close: execute(6, { op: 'close' }),
  control_in: execute(7, { op: 'control_in', setup, length: 64 }),
  control_out: execute(8, { op: 'control_out', setup, data: Uint8Array.of(1, 2, 3) }),
  transfer_in: execute(9, { op: 'transfer_in', endpoint: 0x82, length: 16384 }),
  transfer_out: execute(300, { op: 'transfer_out', endpoint: 1, data: Uint8Array.of(0, 255) }),
  clear_halt: execute(11, { op: 'clear_halt', endpoint: 0x82 }),
  expect: {
    type: 'expect',
    seq: 12,
    of: 9,
    offset: 4,
    bytes: Uint8Array.of(0xaa, 0xbb),
    mask: null,
    length: 8,
    alternatives: [Uint8Array.of(0xcc, 0xdd)],
  },
  expect_masked: {
    type: 'expect',
    seq: 13,
    of: 9,
    offset: 0,
    bytes: Uint8Array.of(0x10),
    mask: Uint8Array.of(0xf0),
    length: null,
    alternatives: [],
  },
  sync: { type: 'sync', seq: 14 },
  result: {
    type: 'result',
    seq: 9,
    completion: { devices: [], data: Uint8Array.of(9, 8, 7), transferred: 3, error: null },
  },
  result_error: {
    type: 'result',
    seq: 10,
    completion: { devices: [], data: empty, transferred: 0, error: 'stall' },
  },
  result_devices: {
    type: 'result',
    seq: 2,
    completion: { devices: [device], data: empty, transferred: 0, error: null },
  },
  expect_failed: { type: 'expect_failed', seq: 12, actual: Uint8Array.of(0xaa, 0xbc) },
  synced: { type: 'synced', seq: 14, discarded: 2 },
  routed: { type: 'routed', stream: 3, message: { type: 'sync', seq: 1 } },
};
await test('every message matches the bytes the Rust types produce, both ways', () => {
  assert.deepEqual(Object.keys(messages).sort(), Object.keys(vectors).sort());
  for (const [name, message] of Object.entries(messages)) {
    assert.equal(hex(encodeMessage(message)), vectors[name], name);
    assert.deepEqual(
      decodeMessage(Uint8Array.from(Buffer.from(vectors[name], 'hex'))),
      message,
      name,
    );
  }
});
await test('payloads are length-prefixed bytes, decoded as views without copies, up to the transfer limit', () => {
  for (const size of [0, 1, 127, 128, 16384, MAX_TRANSFER]) {
    const storage = Uint8Array.from({ length: size + 17 }, (_, i) => i & 255);
    const data = storage.subarray(7, 7 + size);
    const wire = encodeMessage(execute(1, { op: 'transfer_out', endpoint: 1, data }));
    assert.ok(wire.length < size + 20);
    const decoded = decodeMessage(wire) as ExecuteMessage;
    assert.deepEqual(decoded.op.data, data);
    assert.equal(decoded.op.data!.buffer, wire.buffer);
  }
  assert.throws(() =>
    encodeMessage(
      execute(1, { op: 'transfer_out', endpoint: 1, data: new Uint8Array(MAX_TRANSFER + 1) }),
    ),
  );
});
await test('truncated, trailing, unknown, nested and version 3 messages are rejected', () => {
  const wire = encodeMessage(messages.transfer_out);
  assert.throws(() => decodeMessage('text'), /binary/);
  for (let i = 0; i < wire.length; i++) assert.throws(() => decodeMessage(wire.subarray(0, i)));
  assert.throws(() => decodeMessage(Uint8Array.of(...wire, 0)), /Trailing/);
  assert.throws(() => decodeMessage(Uint8Array.of(99)), /variant/);
  assert.throws(() => decodeMessage(Uint8Array.of(16, 1, 16, 1, 12, 1)), /Nested/);
  assert.throws(() => decodeMessage(Buffer.from('FU\x03\0\x02\0\0\0{}', 'latin1')));
  assert.throws(() =>
    encodeMessage(
      execute(1, { op: 'transfer_out', endpoint: 1, data: 'AQID' as unknown as Uint8Array }),
    ),
  );
  assert.throws(() => encodeMessage(execute(1, { op: 'batch' })), /Unknown variant/);
});
await test('expectations compare bytes under an optional mask', () => {
  const data = Uint8Array.of(1, 2, 3, 4);
  assert.ok(expectationHolds(data, 1, Uint8Array.of(2, 3)));
  assert.ok(!expectationHolds(data, 1, Uint8Array.of(2, 4)));
  assert.ok(!expectationHolds(data, 3, Uint8Array.of(4, 5)));
  assert.ok(!expectationHolds(undefined, 0, Uint8Array.of(1)));
  assert.ok(expectationHolds(data, 3, Uint8Array.of(0x14), Uint8Array.of(0x0f)));
  assert.ok(!expectationHolds(data, 3, Uint8Array.of(0x14), Uint8Array.of(0xf0)));
});
class Channel extends EventTarget {
  readyState = 'open';
  bufferedAmount = 0;
  packets: Uint8Array[] = [];
  onmessage?: (event: { data: ArrayBufferLike }) => void;
  send(packet: Uint8Array) {
    this.packets.push(packet);
  }
  close() {
    this.readyState = 'closed';
  }
}
await test('WebRTC chunks and reassembles binary without text conversion and honors backpressure', async () => {
  const tx = new Channel(),
    rx = new Channel();
  const sender = new PeerChannel(
      tx as unknown as RTCDataChannel,
      new EventTarget() as RTCPeerConnection,
    ),
    receiver = new PeerChannel(
      rx as unknown as RTCDataChannel,
      new EventTarget() as RTCPeerConnection,
    );
  const data = Uint8Array.from({ length: 48001 }, (_, i) => i & 255);
  tx.bufferedAmount = 300000;
  const sending = sender.send(data);
  assert.equal(tx.packets.length, 0);
  tx.bufferedAmount = 0;
  tx.dispatchEvent(new Event('bufferedamountlow'));
  await sending;
  assert.equal(tx.packets.length, 4);
  for (const packet of tx.packets) rx.onmessage!({ data: packet.buffer });
  assert.deepEqual(await receiver.receive(), data);
  assert.ok(tx.packets.every((packet) => packet.length <= 16001));
});
