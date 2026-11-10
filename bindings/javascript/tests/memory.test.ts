// Synthetic publication-source loan over the production Rust session and the
// testing Wasm transport. Every expected exchange is computed from the profile
// below: no real firmware, no mocked Session or Memory ownership.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, usb } from '../dist/index.js';
import type { BulkProfile, Memory, MemoryReport, Session } from '../dist/index.js';
import { TestExchange as E, TestScript } from '../pkg/fujimod.js';

await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});

const SCHEMA_URI = 'https://fujimod.com/schemas/bulk.generated.schema.json';
const WINDOW = { address: 0x1000, size: 0x1000 };
const SOURCE_STATE = 0x1200;
const CAPACITY = 32;
const DATA = { address: 0x1800, size: 0x80 };
const CODE = new TextEncoder().encode('stock code');
const TOKEN = 0x1234;
const PACKET_BYTES = 65536;
const BUDGET_BYTES = 65536;

const words = (...values: number[]): Uint8Array => {
  const data = new Uint8Array(values.length * 4);
  const view = new DataView(data.buffer);
  values.forEach((n, i) => view.setUint32(i * 4, n, true));
  return data;
};
const concat = (...arrays: readonly (readonly number[] | Uint8Array)[]) =>
  Uint8Array.from(arrays.flatMap((a) => [...a]));
const ORIGINAL = words(0xdead0000, 16);
const SELECTED = words(0, CAPACITY);
const sha256 = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');

/** One tiny bounded publication profile; nothing here is a camera fact. */
function profileJson(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    $schema: SCHEMA_URI,
    schema: 1,
    id: 'sample-1-bulk',
    firmware_id: 'sample-1',
    product: 'SYNTHETIC',
    internal_version: '0.0',
    code_alias: 0,
    source: { mechanism: 'publication', source_state: SOURCE_STATE, capacity: CAPACITY },
    window: WINDOW,
    code_checks: [
      { name: 'entry', address: WINDOW.address, size: CODE.length, sha256: sha256(CODE) },
    ],
    probes: [{ address: WINDOW.address, size: 4 }],
    scopes: [
      {
        name: 'quick',
        basis: 'Synthetic test range.',
        ranges: [{ name: 'data', address: DATA.address, size: DATA.size }],
      },
    ],
    ...overrides,
  });
}
const PROFILE = usb.BulkProfile.fromJson(profileJson());
const LIMITS = new usb.FirmwareLimits(64, 64, 256, 256);

const env = (kind: number, size: number, offset = 0, token = TOKEN): Uint8Array => {
  const data = new Uint8Array(12);
  const view = new DataView(data.buffer);
  data[0] = kind;
  view.setUint32(2, offset);
  view.setUint32(6, size);
  view.setUint16(10, token);
  return data;
};
const command = (request: Uint8Array, maximum: number, response: Uint8Array): E[] => [
  E.control(env(0xd0, request.length)),
  E.write(request),
  E.control(env(0xd1, maximum)),
  E.read(maximum, response),
];
const memoryRequest = (fn: number, address: number, size: number, payload: Uint8Array = new Uint8Array()) =>
  concat(words(0x200001, 0, 0, 0, fn, 0, 0, address, size), payload);
const readMemory = (address: number, data: Uint8Array): E[] => {
  const request = memoryRequest(2, address, data.length);
  return command(request, request.length + data.length, concat(request, data));
};
const writeMemory = (address: number, data: Uint8Array): E[] => {
  const request = memoryRequest(3, address, data.length, data);
  const reply = memoryRequest(3, address, data.length);
  return command(request, reply.length, reply);
};
const dataRead = (offset: number, data: Uint8Array): E[] => [
  E.control(env(0xe1, data.length, offset)),
  E.read(data.length, data),
];
const info = (selector: number, text: string): E[] => {
  const request = new Uint8Array(32);
  const view = new DataView(request.buffer);
  view.setUint32(0, 0x150000, true);
  view.setUint32(16, selector, true);
  const reply = concat(request.slice(0, 16), Uint8Array.from(Buffer.from(text, 'utf16le')), [0, 0]);
  return command(request, 128, reply);
};
const pendingMemoryRead = (address: number, size: number): E[] => [
  E.control(env(0xd0, 36)),
  E.write(memoryRequest(2, address, size)),
  E.control(env(0xd1, 36 + size)),
  E.pendingRead(36 + size),
];
const pendingMemoryWrite = (address: number, data: Uint8Array): E[] => [
  E.control(env(0xd0, 36 + data.length)),
  E.write(memoryRequest(3, address, data.length, data)),
  E.control(env(0xd1, 36)),
  E.pendingRead(36),
];
const canary = (): E => E.control(env(0xaa, 0));
const closeSteps = (): E[] => [E.control(env(0xc4, 2)), E.read(2, new Uint8Array([0xaa, 0xbb]))];
const pingSteps = (): E[] => [E.control(env(0xc2, 2)), E.read(2, new Uint8Array([0x6f, 0x6b]))];

async function opened(steps: E[] = []): Promise<{ session: Session; script: TestScript }> {
  const raw = new Uint8Array(8);
  const view = new DataView(raw.buffer);
  view.setUint16(0, TOKEN, true);
  view.setUint32(4, PACKET_BYTES);
  const script = new TestScript([E.control(env(0xc0, 8, 0, 0)), E.read(8, raw), ...steps]);
  const session = await script.open(LIMITS, new usb.TransferOptions(100, BUDGET_BYTES));
  return { session, script };
}

/** Identity, stock-code check and the probe's slow/E1 comparison. */
const qualified = (): E[] => [
  ...info(4, 'SYNTHETIC'),
  ...info(8, '0.0'),
  ...readMemory(WINDOW.address, CODE),
];
/** Qualification plus the publication pair's original bytes, install and readback. */
const publicationOpen = (): E[] => [
  ...qualified(),
  ...readMemory(SOURCE_STATE, ORIGINAL),
  ...writeMemory(SOURCE_STATE, SELECTED),
  ...readMemory(SOURCE_STATE, SELECTED),
  ...readMemory(WINDOW.address, CODE.slice(0, 4)),
  ...readMemory(SOURCE_STATE, SELECTED),
  ...dataRead(WINDOW.address, CODE.slice(0, 4)),
  ...readMemory(SOURCE_STATE, SELECTED),
];
const restoreSteps = (): E[] => [
  ...readMemory(SOURCE_STATE, SELECTED),
  ...writeMemory(SOURCE_STATE, ORIGINAL),
  ...readMemory(SOURCE_STATE, ORIGINAL),
];
/** E1 chunks of DATA, each preceded and followed by the owner check. */
function e1Steps(): { steps: E[]; data: Uint8Array } {
  const data = Uint8Array.from({ length: DATA.size }, (_, index) => index & 0xff);
  const steps: E[] = [];
  for (let offset = 0; offset < data.length; offset += CAPACITY) {
    const chunk = data.slice(offset, offset + CAPACITY);
    steps.push(...readMemory(SOURCE_STATE, SELECTED));
    steps.push(...dataRead(DATA.address + offset, chunk));
    steps.push(...readMemory(SOURCE_STATE, SELECTED));
  }
  return { steps, data };
}
const report = (memory: Memory): MemoryReport => memory.state;
const sessionReport = (session: Session): MemoryReport => {
  const value = session.memoryState;
  assert.ok(value, 'the session should report its attached loan');
  return value;
};
const kind =
  (expected: (typeof usb.ErrorKind)[keyof typeof usb.ErrorKind]) => (value: unknown) =>
    usb.isError(value) && value.kind === expected;
async function until(predicate: () => boolean) {
  for (let n = 0; n < 50; n++) {
    if (predicate()) return;
    await new Promise(setImmediate);
  }
  assert.fail('operation did not reach expected pending stage');
}

await test(
  'one loan qualifies, reads, restores and refuses ordinary session work',
  { timeout: 3000 },
  async () => {
    const { steps: readSteps, data } = e1Steps();
    const { session, script } = await opened([
      ...publicationOpen(),
      ...readMemory(WINDOW.address, CODE.slice(0, 4)),
      ...readSteps,
      ...restoreSteps(),
      ...pingSteps(),
      ...closeSteps(),
    ]);
    const profile = usb.BulkProfile.fromJson(profileJson());
    assert.equal(profile.id, 'sample-1-bulk');
    assert.equal(profile.firmwareId, 'sample-1');
    const canonical = JSON.parse(profile.json) as Record<string, unknown>;
    assert.equal(canonical.id, 'sample-1-bulk');
    assert.equal(canonical.firmware_id, 'sample-1');

    const memory = await session.memory(profile);
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    const borrowed = sessionReport(session);
    assert.equal(borrowed.phase, 'Ready');
    assert.equal(borrowed.inFlight, false);
    assert.equal(borrowed.closed, false);
    assert.equal(borrowed.abandoned, false);
    assert.equal(borrowed.loanAvailable, true);
    const loan = borrowed.loan;
    assert.ok(loan);
    assert.equal(loan.address, BigInt(SOURCE_STATE));
    assert.deepEqual(loan.original, ORIGINAL);
    assert.deepEqual(loan.selected, SELECTED);
    assert.equal(report(memory).phase, 'Ready');
    assert.deepEqual(memory.overwritten, { address: SOURCE_STATE, size: 8 });

    const quiet = script.remaining;
    assert.throws(() => session.ping(), kind(usb.ErrorKind.Busy));
    assert.throws(() => session.readSinglePacket(WINDOW.address, 4), kind(usb.ErrorKind.Busy));
    assert.throws(
      () => session.writeSinglePacket(WINDOW.address, new Uint8Array(4)),
      kind(usb.ErrorKind.Busy),
    );
    assert.throws(() => session.close(), kind(usb.ErrorKind.Busy));
    assert.throws(
      () =>
        usb.data.readChunks(
          session,
          new usb.DataWindow(0, 0, 8, 4, usb.OffsetMode.Checked),
          0,
          8,
        ),
      kind(usb.ErrorKind.Busy),
    );
    assert.throws(() => session.memory(profile), kind(usb.ErrorKind.Busy));
    assert.equal(script.remaining, quiet);

    // Pure argument checks and an out-of-window E1 range send nothing.
    assert.throws(() => memory.read(-1, 4), kind(usb.ErrorKind.InvalidArgument));
    assert.throws(() => memory.read(WINDOW.address, 0), kind(usb.ErrorKind.InvalidArgument));
    await assert.rejects(memory.read(0x3000, 128), kind(usb.ErrorKind.Workflow));
    assert.equal(script.remaining, quiet);
    assert.equal(report(memory).phase, 'Ready');
    assert.equal(session.state, usb.SessionState.MemoryOwned);

    // A small read is one bounded command on the retained session.
    assert.deepEqual(await memory.read(WINDOW.address, 4), CODE.slice(0, 4));
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    // A larger read crosses E1 chunks, each guarded by the owner check.
    assert.deepEqual(await memory.read(DATA.address, DATA.size), data);
    assert.equal(report(memory).phase, 'Ready');

    const beforeClose = script.remaining;
    await memory.close();
    assert.equal(script.remaining, beforeClose - 12);
    const closed = report(memory);
    assert.equal(closed.closed, true);
    assert.equal(closed.phase, 'Restored');
    assert.equal(closed.loanAvailable, true);
    assert.equal(memory.overwritten, null);
    assert.deepEqual(closed.loan?.original, ORIGINAL);
    assert.deepEqual(closed.loan?.selected, SELECTED);
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(session.memoryState, undefined);

    const afterClose = script.remaining;
    assert.equal(await memory.close(), undefined);
    assert.equal(script.remaining, afterClose);
    assert.throws(() => memory.read(DATA.address, DATA.size), kind(usb.ErrorKind.Closed));
    assert.equal(script.remaining, afterClose);
    assert.deepEqual(await session.ping(), new Uint8Array([0x6f, 0x6b]));
    await session.close();
    assert.equal(script.remaining, 0);
    session.abandon();
  },
);

await test(
  'a known setup refusal returns the usable session and no loan',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([...info(4, 'OTHER'), canary()]);
    await assert.rejects(session.memory(PROFILE), kind(usb.ErrorKind.Workflow));
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(session.memoryState, undefined);
    assert.equal(script.remaining, 1);
    session.abandon();
  },
);

await test(
  'a caller-supplied JSON descriptor is validated like fromJson',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([...publicationOpen(), ...restoreSteps()]);
    const descriptor = { json: profileJson() } as unknown as BulkProfile;
    const memory = await session.memory(descriptor);
    assert.equal(report(memory).phase, 'Ready');
    await memory.close();
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(script.remaining, 0);
    session.abandon();
  },
);

await test(
  'malformed profiles and descriptors refuse before any session use',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([canary()]);
    assert.throws(() => usb.BulkProfile.fromJson('{'), kind(usb.ErrorKind.InvalidArgument));
    assert.throws(
      () => usb.BulkProfile.fromJson(profileJson({ unknown_field: true })),
      kind(usb.ErrorKind.InvalidArgument),
    );
    assert.throws(
      () => usb.BulkProfile.fromJson(profileJson({ window: { address: WINDOW.address, size: 0 } })),
      kind(usb.ErrorKind.InvalidArgument),
    );
    assert.throws(
      () =>
        usb.BulkProfile.fromJson(
          profileJson({
            code_checks: [
              { name: 'entry', address: WINDOW.address, size: CODE.length, sha256: 'AB'.repeat(32) },
            ],
          }),
        ),
      kind(usb.ErrorKind.InvalidArgument),
    );
    const describe = (value: unknown) => value as unknown as BulkProfile;
    assert.throws(() => session.memory(describe({})), kind(usb.ErrorKind.InvalidArgument));
    assert.throws(
      () =>
        session.memory(
          describe({
            get json(): string {
              throw new Error('unreadable descriptor');
            },
          }),
        ),
      kind(usb.ErrorKind.InvalidArgument),
    );
    assert.throws(
      () => session.memory(describe({ json: '{"schema":1}' })),
      kind(usb.ErrorKind.InvalidArgument),
    );
    assert.throws(
      () => session.memory(describe({ json: profileJson({ unknown_field: true }) })),
      kind(usb.ErrorKind.InvalidArgument),
    );
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(session.memoryState, undefined);
    assert.equal(script.remaining, 1);
    session.abandon();
  },
);

await test(
  'a canceled open after the source borrow keeps the actual loan without a handle',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([
      ...qualified(),
      ...readMemory(SOURCE_STATE, ORIGINAL),
      ...writeMemory(SOURCE_STATE, SELECTED),
      ...pendingMemoryRead(SOURCE_STATE, 8),
      canary(),
    ]);
    const pending = session.memory(PROFILE);
    await until(() => script.remaining === 1);
    const inFlight = sessionReport(session);
    assert.equal(inFlight.phase, 'InFlight');
    assert.equal(inFlight.inFlight, true);
    assert.equal(inFlight.loanAvailable, false);
    assert.equal(inFlight.loan, undefined);
    assert.equal(session.state, usb.SessionState.InFlight);

    session.cancelPending();
    await assert.rejects(pending, kind(usb.ErrorKind.Canceled));
    const settled = sessionReport(session);
    assert.equal(settled.phase, 'Unresolved');
    assert.equal(settled.inFlight, false);
    assert.equal(settled.loanAvailable, true);
    assert.deepEqual(settled.loan?.original, ORIGINAL);
    assert.deepEqual(settled.loan?.selected, SELECTED);
    assert.equal(session.state, usb.SessionState.CompletionUnknown);
    assert.throws(() => session.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.equal(script.remaining, 1);

    session.abandon();
    assert.equal(sessionReport(session).phase, 'Abandoned');
    assert.equal(session.state, usb.SessionState.Abandoned);
    assert.equal(script.remaining, 1);
  },
);

await test(
  'a cancel before the first exchange leaves the guarded setup staged',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([canary()]);
    const pending = session.memory(PROFILE);
    session.cancelPending();
    await assert.rejects(pending, kind(usb.ErrorKind.Canceled));
    const staged = sessionReport(session);
    assert.equal(staged.phase, 'Staged');
    assert.equal(staged.inFlight, false);
    assert.equal(staged.loanAvailable, true);
    assert.equal(staged.loan, undefined);
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    assert.equal(script.remaining, 1);
    session.abandon();
    assert.equal(session.state, usb.SessionState.Abandoned);
    assert.equal(script.remaining, 1);
  },
);

await test(
  'a canceled small read and a canceled small write fence the loan',
  { timeout: 3000 },
  async () => {
    const read = await opened([
      ...publicationOpen(),
      ...pendingMemoryRead(WINDOW.address, 4),
      canary(),
    ]);
    const readMemoryLease = await read.session.memory(PROFILE);
    const pendingRead = readMemoryLease.read(WINDOW.address, 4);
    assert.equal(read.session.state, usb.SessionState.InFlight);
    assert.equal(report(readMemoryLease).inFlight, true);
    assert.equal(report(readMemoryLease).loanAvailable, false);
    await until(() => read.script.remaining === 1);
    read.session.cancelPending();
    await assert.rejects(pendingRead, kind(usb.ErrorKind.Canceled));
    const cancelled = report(readMemoryLease);
    assert.equal(cancelled.phase, 'Unresolved');
    assert.equal(cancelled.closed, false);
    assert.deepEqual(cancelled.loan?.original, ORIGINAL);
    assert.deepEqual(cancelled.loan?.selected, SELECTED);
    assert.equal(read.session.state, usb.SessionState.CompletionUnknown);
    const readQuiet = read.script.remaining;
    assert.throws(() => readMemoryLease.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.equal(read.script.remaining, readQuiet);
    assert.throws(
      () => readMemoryLease.read(WINDOW.address, 4),
      kind(usb.ErrorKind.CompletionUnknown),
    );
    assert.equal(read.script.remaining, readQuiet);
    assert.throws(
      () => readMemoryLease.write(WINDOW.address, new Uint8Array(4)),
      kind(usb.ErrorKind.CompletionUnknown),
    );
    assert.equal(read.script.remaining, readQuiet);
    assert.equal(read.session.state, usb.SessionState.CompletionUnknown);

    const write = await opened([
      ...publicationOpen(),
      ...pendingMemoryWrite(WINDOW.address, new Uint8Array([1, 2, 3, 4])),
      canary(),
    ]);
    const writeLease = await write.session.memory(PROFILE);
    const pendingWrite = writeLease.write(WINDOW.address, new Uint8Array([1, 2, 3, 4]));
    await until(() => write.script.remaining === 1);
    write.session.cancelPending();
    await assert.rejects(pendingWrite, kind(usb.ErrorKind.Canceled));
    const failed = report(writeLease);
    assert.equal(failed.phase, 'Unresolved');
    assert.equal(failed.closed, false);
    assert.deepEqual(failed.loan?.original, ORIGINAL);
    assert.deepEqual(failed.loan?.selected, SELECTED);
    assert.equal(write.session.state, usb.SessionState.CompletionUnknown);
    const writeQuiet = write.script.remaining;
    assert.throws(() => writeLease.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.equal(write.script.remaining, writeQuiet);
    assert.throws(
      () => writeLease.write(WINDOW.address, new Uint8Array(4)),
      kind(usb.ErrorKind.CompletionUnknown),
    );
    assert.equal(write.script.remaining, writeQuiet);
    assert.throws(
      () => writeLease.read(WINDOW.address, 4),
      kind(usb.ErrorKind.CompletionUnknown),
    );
    assert.equal(write.script.remaining, writeQuiet);
    assert.equal(write.session.state, usb.SessionState.CompletionUnknown);
  },
);

await test(
  'a canceled E1 read keeps the report and refuses restoration',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([
      ...publicationOpen(),
      ...readMemory(SOURCE_STATE, SELECTED),
      E.control(env(0xe1, CAPACITY, DATA.address)),
      E.pendingRead(CAPACITY),
      canary(),
    ]);
    const memory = await session.memory(PROFILE);
    const pending = memory.read(DATA.address, DATA.size);
    assert.equal(session.state, usb.SessionState.InFlight);
    await until(() => script.remaining === 1);
    session.cancelPending();
    await assert.rejects(pending, kind(usb.ErrorKind.Canceled));
    const cancelled = report(memory);
    assert.equal(cancelled.phase, 'Unresolved');
    assert.equal(cancelled.closed, false);
    assert.deepEqual(cancelled.loan?.original, ORIGINAL);
    assert.deepEqual(cancelled.loan?.selected, SELECTED);
    assert.equal(session.state, usb.SessionState.CompletionUnknown);
    assert.throws(() => memory.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.throws(() => memory.read(DATA.address, 4), kind(usb.ErrorKind.CompletionUnknown));
    assert.equal(script.remaining, 1);
  },
);

await test(
  'a canceled restore keeps the resource and its report',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([
      ...publicationOpen(),
      ...pendingMemoryRead(SOURCE_STATE, 8),
      canary(),
    ]);
    const memory = await session.memory(PROFILE);
    const pending = memory.close();
    await until(() => script.remaining === 1);
    session.cancelPending();
    await assert.rejects(pending, kind(usb.ErrorKind.Canceled));
    assert.equal(report(memory).phase, 'Unresolved');
    assert.equal(report(memory).closed, false);
    assert.throws(() => memory.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.notEqual(session.memoryState, undefined);
    assert.equal(script.remaining, 1);
  },
);

await test(
  'a failed restore keeps unresolved state and sends no retry',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([
      ...publicationOpen(),
      E.control(env(0xd0, 36)),
      E.write(memoryRequest(2, SOURCE_STATE, 8)),
      E.control(env(0xd1, 44)),
      E.disconnected(44, new Uint8Array([1, 2])),
      canary(),
    ]);
    const memory = await session.memory(PROFILE);
    await assert.rejects(memory.close(), (failure: unknown) => {
      assert.ok(usb.isError(failure));
      assert.equal(failure.kind, usb.ErrorKind.Transfer);
      return true;
    });
    assert.equal(report(memory).phase, 'Unresolved');
    assert.equal(report(memory).closed, false);
    assert.equal(session.state, usb.SessionState.CompletionUnknown);
    assert.throws(() => memory.close(), kind(usb.ErrorKind.CompletionUnknown));
    assert.equal(script.remaining, 1);
  },
);

await test(
  'abandon while idle and while in flight keeps the owner and sends nothing',
  { timeout: 3000 },
  async () => {
    const idle = await opened([...publicationOpen(), canary()]);
    const memory = await idle.session.memory(PROFILE);
    idle.session.abandon();
    const terminal = report(memory);
    assert.equal(terminal.abandoned, true);
    assert.equal(terminal.phase, 'Abandoned');
    assert.deepEqual(terminal.loan?.original, ORIGINAL);
    assert.equal(idle.session.state, usb.SessionState.Abandoned);
    assert.throws(() => idle.session.ping(), kind(usb.ErrorKind.Abandoned));
    assert.throws(() => idle.session.close(), kind(usb.ErrorKind.Abandoned));
    assert.throws(() => idle.session.memory(PROFILE), kind(usb.ErrorKind.Abandoned));
    assert.throws(() => memory.close(), kind(usb.ErrorKind.Abandoned));
    assert.throws(() => memory.read(DATA.address, DATA.size), kind(usb.ErrorKind.Abandoned));
    assert.equal(idle.script.remaining, 1);

    const active = await opened([
      ...publicationOpen(),
      ...pendingMemoryRead(SOURCE_STATE, 8),
      canary(),
    ]);
    const inFlight = await active.session.memory(PROFILE);
    const pending = inFlight.close();
    await until(() => active.script.remaining === 1);
    active.session.abandon();
    await assert.rejects(pending, kind(usb.ErrorKind.Abandoned));
    const settled = report(inFlight);
    assert.equal(settled.abandoned, true);
    assert.equal(settled.phase, 'Abandoned');
    assert.deepEqual(settled.loan?.original, ORIGINAL);
    assert.deepEqual(settled.loan?.selected, SELECTED);
    assert.equal(active.session.state, usb.SessionState.Abandoned);
    assert.equal(active.script.remaining, 1);
  },
);

await test(
  'freeing the handle sends nothing and leaves the loan with the session',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([...publicationOpen(), canary()]);
    const memory = await session.memory(PROFILE);
    memory.free();
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    const held = sessionReport(session);
    assert.deepEqual(held.loan?.original, ORIGINAL);
    assert.equal(script.remaining, 1);
    assert.throws(() => session.close(), kind(usb.ErrorKind.Busy));
    assert.equal(script.remaining, 1);
    session.abandon();
    assert.equal(session.state, usb.SessionState.Abandoned);
    assert.equal(script.remaining, 1);
  },
);

await test(
  'a closed handle cannot close a newer loan',
  { timeout: 3000 },
  async () => {
    const { session, script } = await opened([
      ...publicationOpen(),
      ...restoreSteps(),
      ...publicationOpen(),
      ...restoreSteps(),
    ]);
    const first = await session.memory(PROFILE);
    await first.close();
    const second = await session.memory(PROFILE);
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    const quiet = script.remaining;
    assert.equal(await first.close(), undefined);
    assert.equal(script.remaining, quiet);
    assert.equal(session.state, usb.SessionState.MemoryOwned);
    assert.throws(() => first.read(DATA.address, 4), kind(usb.ErrorKind.Closed));
    assert.equal(script.remaining, quiet);
    await second.close();
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(script.remaining, 0);
    session.abandon();
  },
);
