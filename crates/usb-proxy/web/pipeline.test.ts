import test from 'node:test';
import assert from 'node:assert/strict';
import { Pipeline } from './pipeline.ts';
import type { Completion, Message, ExecuteMessage } from './types.ts';
await test('a pipeline runs in order, checks expectations and discards after a failure until sync', async () => {
  const ran: number[] = [];
  const pipeline = new Pipeline(async (m: ExecuteMessage): Promise<Completion> => {
    ran.push(m.seq);
    if (m.op.op === 'transfer_in') return { data: Uint8Array.of(m.seq, 0x5a, 3), transferred: 3 };
    return { transferred: m.op.data?.length ?? 0, error: m.op.endpoint === 3 ? 'stall' : null };
  });
  const execute = (seq: number, op: ExecuteMessage['op']): Message => ({
    type: 'execute',
    seq,
    device: '1',
    timeout_ms: 1000,
    op,
  });
  const read = (seq: number) => execute(seq, { op: 'transfer_in', endpoint: 0x82, length: 64 });
  const write = (seq: number, endpoint = 1) =>
    execute(seq, { op: 'transfer_out', endpoint, data: Uint8Array.of(seq) });
  const expect = (
    seq: number,
    of: number,
    bytes: number[],
    mask?: number[],
    length?: number,
  ): Message => ({
    type: 'expect',
    seq,
    of,
    offset: 1,
    bytes: Uint8Array.from(bytes),
    mask: mask && Uint8Array.from(mask),
    length,
  });
  const replies = [];
  for (const message of [
    read(1),
    expect(2, 1, [0x5a, 3]),
    expect(3, 1, [0x50], [0xf0]),
    write(4),
    expect(5, 1, [0x5b]),
    write(6),
    read(7),
    { type: 'sync', seq: 8 } as Message,
    write(9, 3),
    write(10),
    { type: 'sync', seq: 11 } as Message,
    write(12),
    expect(13, 99, [1]),
    { type: 'sync', seq: 14 } as Message,
  ])
    replies.push(await pipeline.handle(message));
  assert.deepEqual(ran, [1, 4, 9, 12]);
  assert.deepEqual(
    replies.map((r) => r && { ...r, completion: undefined }),
    [
      { type: 'result', seq: 1, completion: undefined },
      undefined,
      undefined,
      { type: 'result', seq: 4, completion: undefined },
      { type: 'expect_failed', seq: 5, actual: Uint8Array.of(0x5a), completion: undefined },
      undefined,
      undefined,
      { type: 'synced', seq: 8, discarded: 2, completion: undefined },
      { type: 'result', seq: 9, completion: undefined },
      undefined,
      { type: 'synced', seq: 11, discarded: 1, completion: undefined },
      { type: 'result', seq: 12, completion: undefined },
      { type: 'expect_failed', seq: 13, actual: new Uint8Array(), completion: undefined },
      { type: 'synced', seq: 14, discarded: 0, completion: undefined },
    ],
  );
});

await test('reply guards require the exact extent and one complete allowed alternative', async () => {
  for (const [data, accepted] of [
    [Uint8Array.of(0, 0), true],
    [Uint8Array.of(1, 255), true],
    [Uint8Array.of(1, 0), false],
    [Uint8Array.of(0, 255), false],
    [Uint8Array.of(0), false],
    [Uint8Array.of(0, 0, 0), false],
  ] as const) {
    const ran: number[] = [];
    const pipeline = new Pipeline(async (m) => {
      ran.push(m.seq);
      return { data, transferred: data.length };
    });
    await pipeline.handle({
      type: 'execute',
      seq: 1,
      device: '1',
      timeout_ms: 10,
      op: { op: 'transfer_in', endpoint: 0x82, length: 64 },
    });
    const guard = await pipeline.handle({
      type: 'expect',
      seq: 2,
      of: 1,
      offset: 0,
      bytes: Uint8Array.of(0, 0),
      alternatives: [Uint8Array.of(1, 255)],
      length: 2,
    });
    await pipeline.handle({
      type: 'execute',
      seq: 3,
      device: '1',
      timeout_ms: 10,
      op: { op: 'transfer_out', endpoint: 1, data: Uint8Array.of(9) },
    });
    assert.deepEqual(ran, accepted ? [1, 3] : [1]);
    assert.equal(guard?.type, accepted ? undefined : 'expect_failed');
  }
});

await test('a short successful OUT completion prevents dependent USB operations', async () => {
  const ran: number[] = [];
  const pipeline = new Pipeline(async (m) => {
    ran.push(m.seq);
    return { transferred: 1 };
  });
  const completion = await pipeline.handle({
    type: 'execute',
    seq: 1,
    device: '1',
    timeout_ms: 10,
    op: { op: 'transfer_out', endpoint: 1, data: Uint8Array.of(1, 2) },
  });
  assert.equal(completion?.type === 'result' && completion.completion.error, 'short OUT transfer');
  await pipeline.handle({
    type: 'execute',
    seq: 2,
    device: '1',
    timeout_ms: 10,
    op: { op: 'transfer_in', endpoint: 0x82, length: 64 },
  });
  assert.deepEqual(ran, [1]);
});
