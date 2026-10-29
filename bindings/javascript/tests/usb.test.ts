import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, usb, catalog } from '../dist/index.js';
import { TestExchange as E, TestScript } from '../pkg/fujimod.js';
await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});
const words = (...values: number[]) => {
  const data = new Uint8Array(values.length * 4);
  const view = new DataView(data.buffer);
  values.forEach((n, i) => view.setUint32(i * 4, n, true));
  return data;
};
const concat = (...arrays: readonly (readonly number[] | Uint8Array)[]) =>
  Uint8Array.from(arrays.flatMap((a) => [...a]));
const env = (kind: number, size: number, offset = 0, token = 0x1234) => {
  const data = new Uint8Array(12);
  const v = new DataView(data.buffer);
  data[0] = kind;
  v.setUint32(2, offset);
  v.setUint32(6, size);
  v.setUint16(10, token);
  return data;
};
const command = (request: Uint8Array, maximum: number, response: Uint8Array) => [
  E.control(env(0xd0, request.length)),
  E.write(request),
  E.control(env(0xd1, maximum)),
  E.read(maximum, response),
];
const memoryRequest = (fn: number, address: number, length: number, payload = new Uint8Array()) =>
  concat(words(0x200001, 0, 0, 0, fn, 0, 0, address, length), payload);
const readMemory = (address: number, data: Uint8Array) => {
  const req = memoryRequest(2, address, data.length);
  return command(req, 36 + data.length, concat(req, data));
};
const closeSteps = () => [E.control(env(0xc4, 2)), E.read(2, new Uint8Array([0xaa, 0xbb]))];
const limits = (read = 4, write = 4) => new usb.FirmwareLimits(read, write, 256, 256);
async function opened(
  steps: E[],
  {
    packet = 4,
    firmware = limits(),
    budget = 1024,
    trace,
  }: {
    packet?: number;
    firmware?: InstanceType<typeof usb.FirmwareLimits>;
    budget?: number;
    trace?: InstanceType<typeof usb.tracing.Trace>;
  } = {},
) {
  const raw = new Uint8Array([0x34, 0x12, 0, 0, 0, 0, 0, 0]);
  new DataView(raw.buffer).setUint32(4, packet);
  const script = new TestScript([E.control(env(0xc0, 8, 0, 0)), E.read(8, raw), ...steps]);
  const options = new usb.TransferOptions(100, budget);
  if (trace) options.setTrace(trace);
  const session = await script.open(firmware, options);
  return { session, script };
}
const kind = (expected: (typeof usb.ErrorKind)[keyof typeof usb.ErrorKind]) => (value: unknown) =>
  usb.isError(value) && value.kind === expected;
async function until(predicate: () => boolean) {
  for (let n = 0; n < 50; n++) {
    if (predicate()) return;
    await new Promise(setImmediate);
  }
  assert.fail('operation did not reach expected pending stage');
}

await test('quantities reject negative, fractional, overflowing and nonfinite values', () => {
  for (const value of [
    -1,
    0.5,
    2 ** 32,
    NaN,
    Infinity,
    '1',
    false,
    null,
    undefined,
    {},
    2n,
  ] as const) {
    assert.throws(
      // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
      () => new usb.FirmwareLimits(value, 1, 1, 1),
      kind(usb.ErrorKind.InvalidArgument),
    );
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => new usb.research.CommandCode(value), kind(usb.ErrorKind.InvalidArgument));
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => new usb.TransferOptions(value, 1024), kind(usb.ErrorKind.InvalidArgument));
  }
  assert.throws(() => new usb.InterfaceSelection(256, 0, 0), kind(usb.ErrorKind.InvalidArgument));
  assert.throws(() => new usb.app80.CommandCode(256), kind(usb.ErrorKind.InvalidArgument));
  assert.throws(
    () => new usb.research.Request(new usb.app80.CommandCode(3), new Uint8Array(16), 32),
  );
});

await test('one direct read, immutable metadata and raw close acknowledgment', async () => {
  const trace = new usb.tracing.Trace(64, 1024),
    expected = new Uint8Array([1, 2, 3, 4]);
  const { session, script } = await opened([...readMemory(0x12345678, expected), ...closeSteps()], {
    trace,
  });
  assert.equal(session.state, usb.SessionState.Ready);
  assert.equal(session.packetBytes, 4);
  assert.equal(session.sessionId, 0x1234);
  assert.throws(() => {
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    session.packetBytes = 128;
  }, TypeError);
  assert.deepEqual(await session.readSinglePacket(0x12345678, 4), expected);
  assert.deepEqual(await session.close(), new Uint8Array([0xaa, 0xbb]));
  assert.equal(session.state, usb.SessionState.Closed);
  session.abandon();
  assert.equal(session.state, usb.SessionState.Closed);
  assert.equal(script.remaining, 0);
  const batch = trace.drain();
  assert.equal(batch.droppedEvents, 0n);
  assert.equal(batch.droppedBytes, 0n);
  assert.ok(batch.events.length > 0);
  assert.equal(typeof batch.events[0].sequence, 'bigint');
  assert.ok(batch.events.every((e) => e.timestampMs > 1_500_000_000_000));
});

await test('asynchronous failures carry the stack of the call that started them', async () => {
  const { session } = await opened([]);
  const failure = await session.readSinglePacket(0x1000, 4).then(
    () => assert.fail('the unscripted read succeeded'),
    (error: unknown) => error,
  );
  assert.ok(usb.isError(failure));
  assert.match(failure.stack ?? '', /--- operation started ---\n[^]*usb\.test\.ts/);
  session.abandon();
});

await test('single-packet commands refuse lengths beyond the advertised packet', async () => {
  const { session, script } = await opened([]);
  assert.throws(() => session.readSinglePacket(0x1000, 5), (error: unknown) => {
    assert.ok(usb.isError(error));
    assert.equal(error.kind, usb.ErrorKind.InvalidArgument);
    assert.match(error.message, /at most 4 bytes; use session\.memory\(firmwareId\)/);
    return true;
  });
  assert.throws(
    () => session.writeSinglePacket(0x1000, new Uint8Array(5)),
    kind(usb.ErrorKind.InvalidArgument),
  );
  assert.equal(script.remaining, 0);
  session.abandon();
});

await test('range ownership spans chunks and for-await break performs no next read', async () => {
  const { session, script } = await opened([
    E.control(env(0xe1, 4, 0)),
    E.read(4, new Uint8Array([1, 2, 3, 4])),
    ...closeSteps(),
  ]);
  const reader = usb.data.readChunks(
    session,
    new usb.DataWindow(0x1000, 0x1000, 12, 4, usb.OffsetMode.Checked),
    0x1000,
    12,
  );
  assert.equal(session.state, usb.SessionState.ReadingRange);
  assert.throws(() => session.ping(), kind(usb.ErrorKind.Busy));
  for await (const data of reader) {
    assert.deepEqual(data, new Uint8Array([1, 2, 3, 4]));
    break;
  }
  assert.equal(reader.bytesCompleted, 4);
  assert.equal(session.state, usb.SessionState.Ready);
  assert.equal(script.remaining, 2);
  await session.close();
  assert.equal(script.remaining, 0);
});

await test('bulk failure retains completed chunks and partial bytes without cleanup', async () => {
  const { session, script } = await opened([
    E.control(env(0xe1, 4, 0)),
    E.read(4, new Uint8Array([1, 2, 3, 4])),
    E.control(env(0xe1, 4, 4)),
    E.disconnected(4, new Uint8Array([5, 6])),
  ]);
  const reader = usb.data.readChunks(
    session,
    new usb.DataWindow(0x1000, 0x1000, 12, 4, usb.OffsetMode.Checked),
    0x1000,
    10,
  );
  const first = await reader.next();
  assert.deepEqual(first.value, new Uint8Array([1, 2, 3, 4]));
  await assert.rejects(reader.next(), (e) => {
    assert.ok(usb.isError(e));
    assert.equal(e.kind, usb.ErrorKind.Transfer);
    assert.deepEqual(e.partial, new Uint8Array([5, 6]));
    assert.equal(e.stage, usb.TransferStage.BulkIn);
    assert.equal(e.cause?.transferKind, usb.TransferFailureKind.Disconnected);
    return true;
  });
  assert.equal(reader.bytesCompleted, 4);
  assert.equal(session.state, usb.SessionState.CompletionUnknown);
  await assert.rejects(async () => session.close(), kind(usb.ErrorKind.CompletionUnknown));
  assert.equal(script.remaining, 0);
  assert.deepEqual(first.value, new Uint8Array([1, 2, 3, 4]));
});

await test('abandon a dispatched operation prevents all further transfers', async () => {
  const trace = new usb.tracing.Trace(64, 1024);
  const { session, script } = await opened([E.control(env(0xc2, 2)), E.pendingRead(2)], { trace });
  const pending = session.ping(),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Abandoned));
  assert.equal(session.state, usb.SessionState.InFlight);
  assert.throws(() => session.ping(), kind(usb.ErrorKind.Busy));
  await until(() => script.remaining === 0);
  session.abandon();
  await rejected;
  assert.equal(session.state, usb.SessionState.Abandoned);
  assert.throws(() => session.close(), kind(usb.ErrorKind.Abandoned));
  assert.ok(trace.drain().events.some((e) => e.phase === usb.tracing.Phase.Canceled));
});

await test('abandon before dispatch emits no operation transfer', async () => {
  const { session, script } = await opened([E.control(env(0xc2, 2)), E.pendingRead(2)]);
  const pending = session.ping();
  session.abandon();
  await assert.rejects(pending, kind(usb.ErrorKind.Abandoned));
  assert.equal(script.remaining, 2);
});

await test('abandon an active range read retains explicit abandoned state', async () => {
  const { session, script } = await opened([E.control(env(0xe1, 4, 0)), E.pendingRead(4)]);
  const reader = usb.data.readChunks(
    session,
    new usb.DataWindow(0, 0, 8, 4, usb.OffsetMode.Checked),
    0,
    8,
  );
  const pending = reader.next(),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Abandoned));
  await until(() => script.remaining === 0);
  assert.throws(() => reader.close(), kind(usb.ErrorKind.Busy));
  session.abandon();
  await rejected;
  assert.equal(session.state, usb.SessionState.Abandoned);
  assert.equal(reader.bytesCompleted, 0);
});

await test('a failed close consumes the session and cannot send another C4', async () => {
  const { session, script } = await opened([
    E.control(env(0xc4, 2)),
    E.read(2, new Uint8Array([0x12])),
  ]);
  await assert.rejects(session.close(), (e) => {
    assert.ok(usb.isError(e));
    assert.equal(e.kind, usb.ErrorKind.Protocol);
    assert.equal(e.protocolKind, usb.ProtocolFailureKind.ReplyLength);
    assert.equal(e.expected, 2);
    assert.equal(e.actual, 1);
    return true;
  });
  assert.equal(session.state, usb.SessionState.CompletionUnknown);
  assert.throws(() => session.close(), kind(usb.ErrorKind.CompletionUnknown));
  assert.equal(script.remaining, 0);
});

await test('download Writing is an application enum without a hidden poll or clear', async () => {
  const request = words(0x150000, 0, 0, 0, 0xf0001, 0, 0, 0),
    text = Uint8Array.from(Buffer.from('WRITING\0', 'utf16le'));
  const { session, script } = await opened(
    command(request, 128, concat(words(0x150000, 0, 0, 0), text)),
  );
  const result = await session.firmwareDownloadStatus();
  assert.equal(result.state, usb.FirmwareDownloadState.Writing);
  assert.equal(result.text, 'WRITING');
  assert.equal(session.state, usb.SessionState.Ready);
  assert.equal(script.remaining, 0);
  session.abandon();
});

await test('research device status is a completed reply with preserved raw fields', async () => {
  const request = new usb.research.Request(
      new usb.research.CommandCode(0x1a0001),
      new Uint8Array(16),
      32,
    ),
    raw = words(0x1a0001, 1, 0xffff, 0, 2, 0, 0, 0);
  const { session, script } = await opened(command(words(0x1a0001, 0, 0, 0, 0, 0, 0, 0), 32, raw));
  const result = await session.researchCommand(request);
  assert.equal(result.status, usb.CommandStatus.DeviceError);
  assert.equal(result.primary, 1);
  assert.equal(result.secondary, 0xffff);
  assert.deepEqual(result.raw, raw);
  assert.equal(session.state, usb.SessionState.Ready);
  assert.equal(script.remaining, 0);
  session.abandon();
});

await test('upload holds ownership through setup, every piece and opaque E8', async () => {
  const setup = words(0x760001, 0, 0, 0, 0, 3, 0, 0),
    reply = words(0x760001, 0, 0, 0, 9, 8, 7, 6);
  const steps = [
    ...command(setup, 32, reply),
    E.control(env(0xe0, 2)),
    E.write(new Uint8Array([1, 2])),
    E.control(env(0xe0, 1)),
    E.write(new Uint8Array([3])),
    E.control(env(0xe8, 32)),
    E.read(32, new Uint8Array([4, 5, 6, 7])),
  ];
  const { session, script } = await opened(steps),
    pending = usb.data.upload(session, new Uint8Array([1, 2, 3]), 2, usb.DataWriteMode.Preserve);
  assert.throws(() => session.ping(), kind(usb.ErrorKind.Busy));
  const result = await pending;
  assert.equal(result.bytesWritten, 3);
  assert.deepEqual(result.acknowledgment, new Uint8Array([4, 5, 6, 7]));
  assert.deepEqual(result.setup.fields, new Uint32Array([9, 8, 7, 6]));
  assert.equal(script.remaining, 0);
  session.abandon();
});

await test('App80 uses explicit mailbox operations without polling', async () => {
  const args = Uint8Array.from({ length: 32 }, (_, n) => n),
    payload = concat(new Uint8Array([0x7f]), args);
  const stage = concat(words(0x1a0002, 0, 0, 0, 3, 1, 0, 0x81, 32), args),
    publish = concat(words(0x1a0002, 0, 0, 0, 3, 1, 0, 0x80, 33), payload),
    read = words(0x1a0002, 0, 0, 0, 2, 1, 0, 0x80, 33);
  const { session, script } = await opened([
    ...command(stage, 36, stage.slice(0, 36)),
    ...command(publish, 36, publish.slice(0, 36)),
    ...command(read, 69, concat(read, payload)),
  ]);
  const result = await usb.app80.exchange(
    session,
    new usb.app80.Mailbox(0, 0x80, 256),
    new usb.app80.Request(new usb.app80.CommandCode(0x7f), args),
  );
  assert.deepEqual(result, payload);
  assert.equal(script.remaining, 0);
  session.abandon();
});

await test('invalid enum choices and address types fail before reserving or dispatching', async () => {
  const { session, script } = await opened([]);
  for (const value of ['Product', '2', false, null, undefined, {}, 1.5, 100] as const) {
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => session.deviceInfo(value), kind(usb.ErrorKind.InvalidArgument));
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => session.prepareDataWrite(1, value), kind(usb.ErrorKind.InvalidArgument));
  }
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => session.readSinglePacket('0x1000', 1), kind(usb.ErrorKind.InvalidArgument));
  for (const bytes of [
    '1234',
    [1, 2, 3, 4],
    new Int8Array([1]),
    new Float32Array([1]),
    null,
  ] as const) {
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => session.writeSinglePacket(0x1000, bytes), kind(usb.ErrorKind.InvalidArgument));
  }
  assert.equal(script.remaining, 0);
  assert.equal(session.state, usb.SessionState.Ready);
  session.abandon();
});

await test('abandoning a pending close preserves explicit abandonment without retry', async () => {
  const { session, script } = await opened([E.control(env(0xc4, 2)), E.pendingRead(2)]);
  const pending = session.close(),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Abandoned));
  await until(() => script.remaining === 0);
  session.abandon();
  await rejected;
  assert.equal(session.state, usb.SessionState.Abandoned);
  assert.equal(script.remaining, 0);
});

await test('cancelPending preserves Ready before dispatch and fences dispatched I/O', async () => {
  const early = await opened([]);
  const never = early.session.ping();
  early.session.cancelPending();
  await assert.rejects(never, kind(usb.ErrorKind.Canceled));
  assert.equal(early.session.state, usb.SessionState.Ready);
  assert.equal(early.script.remaining, 0);
  const { session, script } = await opened([E.control(env(0xc2, 2)), E.pendingRead(2)]);
  const pending = session.ping(),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Canceled));
  await until(() => script.remaining === 0);
  session.cancelPending();
  session.cancelPending();
  assert.throws(() => session.ping(), kind(usb.ErrorKind.Busy));
  await rejected;
  assert.equal(session.state, usb.SessionState.CompletionUnknown);
  await assert.rejects(async () => session.close(), kind(usb.ErrorKind.CompletionUnknown));
});

await test('cancelPending on a pending chunk closes the reader and restores fenced ownership', async () => {
  const { session, script } = await opened([E.control(env(0xe1, 4)), E.pendingRead(4)]);
  const reader = usb.data.readChunks(
    session,
    new usb.DataWindow(0x1000, 0x1000, 8, 4, usb.OffsetMode.Checked),
    0x1000,
    8,
  );
  const pending = reader.next(),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Canceled));
  await until(() => script.remaining === 0);
  reader.cancelPending();
  await rejected;
  reader.close();
  assert.deepEqual(await reader.next(), { done: true, value: undefined });
  assert.equal(reader.bytesCompleted, 0);
  assert.equal(session.state, usb.SessionState.CompletionUnknown);
});

function serviceDefinition(
  rows: readonly (readonly [number, string, readonly number[], number | null])[] = [
    [153, 'flush_configuration', [7], 4],
  ],
  expectedId = 'synthetic-b-v2',
) {
  const document = {
    schema: 1,
    firmware: { id: 'synthetic-b-v2', model: 'Body B', version: '2', sha256: 'ab'.repeat(32) },
    mailbox: { page: 3, offset: 0x240, configuration_limit: 512 },
    entries: rows.map(([code, codec, slots, enabled_modes]) => ({
      code,
      label: codec,
      codec,
      slots,
      enabled_modes,
      next_mode: 0,
      firmware_kind: 19,
      handler: 4294967296,
      parameter_table: 8192,
    })),
  };
  return catalog.service.Definition.fromJson(
    JSON.stringify(document),
    new catalog.FirmwareIdentity(expectedId, 'ab'.repeat(32)),
  );
}
function field<K extends string>(value: unknown, key: K): unknown {
  assert.ok(value && typeof value === 'object' && key in value);
  return (value as Record<K, unknown>)[key];
}
const serviceModes = () => new catalog.service.ServiceModes(4);
const saveInvocation = () =>
  new usb.app80.Service(serviceDefinition()).prepare(
    serviceModes(),
    usb.app80.Command.flushConfiguration(),
  );
function serviceExchange(
  invocation: import('../dist/index.js').App80Invocation,
  result: Uint8Array,
) {
  const payload = invocation.raw;
  const stage = concat(words(0x1a0002, 0, 0, 0, 3, 1, 3, 0x241, 32), payload.slice(1)),
    publish = concat(words(0x1a0002, 0, 0, 0, 3, 1, 3, 0x240, 33), payload),
    read = words(0x1a0002, 0, 0, 0, 2, 1, 3, 0x240, 33);
  return [
    ...command(stage, 36, stage.slice(0, 36)),
    ...command(publish, 36, publish.slice(0, 36)),
    ...command(read, 69, concat(read, result)),
  ];
}
await test('shared data loader preserves enums, explicit identity and unreviewed rows', () => {
  const d = serviceDefinition([[153, 'flush_configuration', [7], null]]),
    entry = d.entries[0];
  assert.equal(d.identity.id, 'synthetic-b-v2');
  assert.equal(d.identity.sha256, 'ab'.repeat(32));
  assert.equal(entry.codec, catalog.service.Codec.FlushConfiguration);
  assert.equal(entry.handler, 4294967296n);
  assert.equal(entry.enabledModes.kind, catalog.service.ModeAvailabilityKind.Unreviewed);
  assert.equal(entry.transition.kind, catalog.service.ModeTransitionKind.Keep);
  assert.deepEqual(entry.slots, new Uint8Array([7]));
  assert.equal(entry.firmwareKind, 19);
  assert.equal(d.select(153, serviceModes()), undefined);
  assert.deepEqual(
    [d.mailbox.page, d.mailbox.offset, d.mailbox.configurationLimit],
    [3, 0x240, 512],
  );
  assert.throws(
    () => serviceDefinition(undefined, 'another-body'),
    (e) => catalog.isError(e) && e.reason.kind === catalog.DefinitionFailureKind.FirmwareMismatch,
  );
  assert.throws(
    () => new catalog.FirmwareIdentity('body', 'é'.repeat(32)),
    (e) => catalog.isError(e) && e.reason.kind === catalog.DefinitionFailureKind.InvalidDigest,
  );
  assert.throws(
    () => new catalog.service.ServiceModes(0),
    (e) => catalog.isError(e) && e.reason.kind === catalog.DefinitionFailureKind.EmptyModes,
  );
  assert.throws(
    () => catalog.service.Definition.fromJson('{}', d.identity),
    (e) => catalog.isError(e) && e.reason.kind === catalog.DefinitionFailureKind.Json,
  );
  assert.throws(() => {
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    entry.codec = catalog.service.Codec.Research;
  }, TypeError);
});

await test('typed command selection errors retain enum reasons without fallback', () => {
  const service = new usb.app80.Service(serviceDefinition()),
    modes = serviceModes();
  assert.throws(
    () => new usb.app80.PinBank(16),
    (e) =>
      usb.app80.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.argument === usb.app80.ArgumentKind.PinBank,
  );
  assert.throws(
    () => new usb.app80.WordAddress(3),
    (e) =>
      usb.app80.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.argument === usb.app80.ArgumentKind.WordAlignment,
  );
  assert.throws(
    () => service.prepare(new catalog.service.ServiceModes(1), usb.app80.Command.flushConfiguration()),
    (e) =>
      usb.app80.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Unsupported &&
      e.reason.codec === catalog.service.Codec.FlushConfiguration,
  );
  assert.throws(
    () =>
      service.prepareResearch(
        new usb.app80.CommandCode(153),
        new catalog.service.ServiceModes(1),
        new Uint8Array(),
      ),
    (e) =>
      usb.app80.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Unavailable &&
      e.reason.code === 153,
  );
  assert.throws(
    () => service.prepareResearch(new usb.app80.CommandCode(153), modes, new Uint8Array(33)),
    (e) => usb.app80.isError(e) && e.reason.kind === usb.app80.CommandFailureKind.OversizeArguments,
  );
  const ambiguous = new usb.app80.Service(
    serviceDefinition([
      [153, 'flush_configuration', [7], 4],
      [154, 'flush_configuration', [7], 4],
    ]),
  );
  assert.throws(
    () => ambiguous.prepare(modes, usb.app80.Command.flushConfiguration()),
    (e) => usb.app80.isError(e) && e.reason.kind === usb.app80.CommandFailureKind.Ambiguous,
  );
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => new usb.app80.Command());
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => service.prepare(4, usb.app80.Command.flushConfiguration()));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => usb.app80.Command.readPins(0));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => usb.app80.Command.profileByte('read'));
  for (const value of ['1', false, NaN, Infinity, 1.5, -1, 256] as const)
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => usb.app80.Command.readChannel(value), kind(usb.ErrorKind.InvalidArgument));
  assert.throws(
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    () => usb.app80.ProfileAction.read('First', new usb.app80.ProfileIndex(0)),
    kind(usb.ErrorKind.InvalidArgument),
  );
});

await test('all typed command variants delegate encoding and slot selection to Rust', () => {
  const C = usb.app80.Command,
    P = usb.app80.ProfileAction;
  const cases = [
    [
      'write_pins',
      C.writePins(new usb.app80.PinBank(2), 0x1234, 0x5678),
      [2, 0x12, 0x34, 0x56, 0x78],
    ],
    ['read_pins', C.readPins(new usb.app80.PinBank(2)), [2, 0, 0]],
    ['read_channel', C.readChannel(30), [30, 0, 0]],
    ['write_channel', C.writeChannel(30, 0x1234), [30, 0x12, 0x34]],
    [
      'profile_byte',
      C.profileByte(P.read(usb.app80.ProfileBank.First, new usb.app80.ProfileIndex(3))),
      [1, 3, 0],
    ],
    [
      'profile_byte',
      C.profileByte(P.write(usb.app80.ProfileBank.Second, new usb.app80.ProfileIndex(4), 37)),
      [4, 4, 37],
    ],
    ['profile_byte', C.profileByte(P.reset()), [5, 0, 0]],
    ['download_status', C.downloadStatus(), [0]],
    ['copy_configuration', C.copyConfiguration(9), [9]],
    ['flush_configuration', C.flushConfiguration(), [255]],
    [
      'memory_word',
      C.readWord(new usb.app80.WordAddress(0x40001000)),
      [0, 0, 0x40, 0, 0x10, 0, 0, 0, 0, 0],
    ],
    [
      'memory_word',
      C.writeWord(new usb.app80.WordAddress(0x40001000), 0x12345678),
      [1, 0, 0x40, 0, 0x10, 0, 0x12, 0x34, 0x56, 0x78],
    ],
  ] as const;
  for (const [codec, command, expected] of cases) {
    const slots = expected.map((_, i) => (i * 3 + 7) % 32),
      service = new usb.app80.Service(serviceDefinition([[153, codec, slots, 4]]));
    const invocation = service.prepare(serviceModes(), command),
      raw = invocation.raw;
    assert.equal(raw[0], 153);
    assert.deepEqual(
      slots.map((s) => raw[1 + s]),
      expected,
    );
    assert.equal(invocation.configurationLimit, 512);
    raw[0] = 0;
    assert.equal(invocation.raw[0], 153);
  }
});

await test('typed result unions retain rejection, unknown bytes and immutable payloads', () => {
  const C = usb.app80.Command,
    R = usb.app80.ReplyKind;
  function decode(
    codec: string,
    slots: number[],
    command: import('../pkg/fujimod.js').App80Command,
    changes: Record<string, number> = {},
  ) {
    const invocation = new usb.app80.Service(serviceDefinition([[153, codec, slots, 4]])).prepare(
        serviceModes(),
        command,
      ),
      raw = invocation.raw;
    for (const [position, byte] of Object.entries(changes)) raw[1 + slots[Number(position)]] = byte;
    const reply = invocation.decode(raw);
    assert.deepEqual(reply.raw, raw);
    assert.ok(Object.isFrozen(reply.value));
    return reply.value;
  }
  assert.deepEqual(
    decode('read_pins', [7, 9, 3], C.readPins(new usb.app80.PinBank(2)), { 1: 0x12, 2: 0x34 }),
    { kind: R.Pins, levels: 0x1234 },
  );
  assert.deepEqual(decode('read_channel', [7, 9, 3], C.readChannel(30), { 1: 0x12, 2: 0x34 }), {
    kind: R.Channel,
    value: 0x1234,
  });
  assert.deepEqual(
    decode(
      'profile_byte',
      [7, 9, 3],
      C.profileByte(
        usb.app80.ProfileAction.read(usb.app80.ProfileBank.First, new usb.app80.ProfileIndex(0)),
      ),
      { 2: 57 },
    ),
    { kind: R.ProfileByte, value: 57 },
  );
  assert.deepEqual(decode('download_status', [7], C.downloadStatus(), { 0: 1 }), {
    kind: R.DownloadStatus,
    rawStatus: 1,
  });
  assert.deepEqual(decode('copy_configuration', [7], C.copyConfiguration(1)), {
    kind: R.NoPayload,
  });
  assert.deepEqual(decode('flush_configuration', [7], C.flushConfiguration()), {
    kind: R.ConfigurationSaved,
    status: { kind: usb.app80.SaveStatusKind.Unknown, rawStatus: 255 },
  });
  const read = C.readWord(new usb.app80.WordAddress(0x40001000)),
    write = C.writeWord(new usb.app80.WordAddress(0x40001000), 0x12345678),
    slots = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
  assert.deepEqual(
    decode('memory_word', slots, read, { 1: 1, 6: 0x12, 7: 0x34, 8: 0x56, 9: 0x78 }),
    { kind: R.ReadWord, result: { kind: usb.app80.WordReadKind.Value, value: 0x12345678 } },
  );
  assert.deepEqual(field(decode('memory_word', slots, read, { 1: 0 }), 'result'), {
    kind: usb.app80.WordReadKind.Rejected,
  });
  assert.deepEqual(field(decode('memory_word', slots, read, { 1: 42 }), 'result'), {
    kind: usb.app80.WordReadKind.Unknown,
    rawStatus: 42,
  });
  assert.deepEqual(field(decode('memory_word', slots, write, { 1: 1 }), 'result'), {
    kind: usb.app80.WordWriteKind.Confirmed,
  });
  assert.deepEqual(field(decode('memory_word', slots, write, { 1: 0 }), 'result'), {
    kind: usb.app80.WordWriteKind.Rejected,
  });
  assert.deepEqual(field(decode('memory_word', slots, write, { 1: 42 }), 'result'), {
    kind: usb.app80.WordWriteKind.Unknown,
    rawStatus: 42,
  });
  const invocation = saveInvocation();
  assert.throws(() => invocation.decode(new Uint8Array(33)), kind(usb.ErrorKind.Protocol));
  const research = new usb.app80.Service(serviceDefinition()).prepareResearch(
    new usb.app80.CommandCode(153),
    serviceModes(),
    new Uint8Array(),
  );
  assert.deepEqual(research.decode(new Uint8Array(33)).value, { kind: R.Research });
});

await test('typed invocation awaits publish and status one never starts a poll', async () => {
  const invocation = saveInvocation(),
    raw = invocation.raw;
  raw[8] = 1;
  const { session, script } = await opened(serviceExchange(invocation, raw));
  const pending = usb.app80.execute(session, invocation);
  assert.equal(session.state, usb.SessionState.InFlight);
  const reply = await pending;
  assert.deepEqual(reply.value, {
    kind: usb.app80.ReplyKind.ConfigurationSaved,
    status: { kind: usb.app80.SaveStatusKind.Failed },
  });
  assert.equal(session.state, usb.SessionState.Ready);
  assert.equal(script.remaining, 0);
  session.abandon();
  const small = await opened([], { firmware: new usb.FirmwareLimits(256, 256, 32, 256) });
  assert.throws(
    () => usb.app80.execute(small.session, invocation),
    kind(usb.ErrorKind.InvalidArgument),
  );
  assert.equal(small.session.state, usb.SessionState.Ready);
  assert.equal(small.script.remaining, 0);
  small.session.abandon();
});

await test('typed foreign reply and interrupted publication fence session ownership', async () => {
  const invocation = saveInvocation(),
    bad = await opened(serviceExchange(invocation, new Uint8Array(33)));
  await assert.rejects(
    usb.app80.execute(bad.session, invocation),
    (e) =>
      usb.isError(e) &&
      e.kind === usb.ErrorKind.Protocol &&
      e.protocolKind === usb.ProtocolFailureKind.CommandEcho,
  );
  assert.equal(bad.session.state, usb.SessionState.CompletionUnknown);
  await assert.rejects(bad.session.close(), kind(usb.ErrorKind.CompletionUnknown));
  assert.equal(bad.script.remaining, 0);
  const steps = serviceExchange(invocation, invocation.raw).slice(0, 8);
  steps[7] = E.pendingRead(36);
  const { session, script } = await opened(steps),
    pending = usb.app80.execute(session, invocation),
    rejected = assert.rejects(pending, kind(usb.ErrorKind.Canceled));
  await until(() => script.remaining === 0);
  session.cancelPending();
  await rejected;
  assert.equal(session.state, usb.SessionState.CompletionUnknown);
  assert.throws(() => session.ping(), kind(usb.ErrorKind.CompletionUnknown));
});

await test('peripheral commands preserve typed ports, checked payloads and low-halfword results', () => {
  const S = usb.app80,
    C = S.Command,
    R = S.RegisterAction;
  const definition = serviceDefinition([
    [200, 'register_control', [7, 12, 17, 22, 27, 0, 5], 4],
    [201, 'i2c', Array.from({ length: 14 }, (_, n) => (n * 5 + 7) % 32), 4],
    [202, 'accumulator', [7, 12, 17, 22, 27, 0, 5], 4],
    [203, 'peripheral_report', Array.from({ length: 8 }, (_, n) => (n * 5 + 7) % 32), 4],
  ]);
  const service = new S.Service(definition),
    modes = serviceModes();
  const slots = (codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec]) => {
    const entry = definition.entries.find((e) => e.codec === codec);
    assert.ok(entry);
    return entry.slots;
  };
  const logical = (
    invocation: import('../dist/index.js').App80Invocation,
    codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec],
  ) => [...slots(codec)].map((s) => invocation.raw[1 + s]);
  const decode = (
    invocation: import('../dist/index.js').App80Invocation,
    codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec],
    values: readonly number[],
  ) => {
    const raw = invocation.raw;
    [...slots(codec)].forEach((slot, n) => {
      raw[1 + slot] = values[n];
    });
    const reply = invocation.decode(raw);
    assert.deepEqual(reply.raw, raw);
    return reply.value;
  };
  const K = catalog.service.Codec;
  for (const [action, expected] of [
    [R.audioValue(0x56), [0, 0, 0, 0, 0x56, 0, 0]],
    [R.displayFirst(7, 0x1234), [1, 0, 7, 0x12, 0x34, 0, 0]],
    [R.displayStub(7, 0x1234), [2, 0, 7, 0x12, 0x34, 0, 0]],
    [R.displaySecond(7, 0x1234), [3, 0, 7, 0x12, 0x34, 0, 0]],
    [R.powerWrite(0x1234, 0x56), [4, 0x12, 0x34, 0, 0x56, 0, 0]],
    [R.positionPair(0x1234, 0x5678), [5, 0x12, 0x34, 0x56, 0x78, 0, 0]],
    [R.noopFirst(), [7, 0, 0, 0, 0, 0, 0]],
    [R.noopSecond(), [8, 0, 0, 0, 0, 0, 0]],
  ] as const) {
    const invocation = service.prepare(modes, C.registerControl(action));
    assert.equal(invocation.raw[0], 200);
    assert.deepEqual(logical(invocation, K.RegisterControl), expected);
    assert.deepEqual(invocation.decode(invocation.raw).value, { kind: S.ReplyKind.NoPayload });
  }
  for (const n of [0, 11] as const) {
    assert.throws(
      () => new S.I2cLength(n),
      (e) =>
        S.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === S.ArgumentKind.I2cLength,
    );
    assert.throws(
      () => new S.I2cPayload(new Uint8Array(n)),
      (e) =>
        S.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === S.ArgumentKind.I2cLength,
    );
  }
  assert.equal(new S.I2cLength(10).value, 10);
  assert.equal(new S.I2cPayload(new Uint8Array(10)).data.length, 10);
  assert.equal('Port3' in S.I2cPort, false);
  for (const [port, selector] of [
    [S.I2cPort.Port0, 0],
    [S.I2cPort.Port1, 1],
    [S.I2cPort.Port2, 2],
    [S.I2cPort.Port4, 4],
    [S.I2cPort.Port5, 5],
  ] as const) {
    const read = service.prepare(modes, C.i2c(S.I2cAction.read(port, 0xa5, new S.I2cLength(3))));
    assert.equal(read.raw[0], 201);
    assert.deepEqual(logical(read, K.I2c), [0, selector, 3, 0xa5, ...Array(10).fill(0)]);
    assert.deepEqual(decode(read, K.I2c, [0, selector, 3, 0xa5, 1, 2, 3, ...Array(7).fill(99)]), {
      kind: S.ReplyKind.I2cData,
      data: new Uint8Array([1, 2, 3]),
    });
    const original = new Uint8Array([4, 5, 6]),
      payload = new S.I2cPayload(original);
    original.fill(0);
    assert.deepEqual(payload.data, new Uint8Array([4, 5, 6]));
    const write = service.prepare(modes, C.i2c(S.I2cAction.write(port, 0x12, payload)));
    assert.deepEqual(logical(write, K.I2c), [1, selector, 3, 0x12, 4, 5, 6, ...Array(7).fill(0)]);
    assert.deepEqual(write.decode(write.raw).value, { kind: S.ReplyKind.NoPayload });
  }
  const single = service.prepare(modes, C.i2c(S.I2cAction.writeSingle(0xa5, 0x7f)));
  assert.deepEqual(logical(single, K.I2c), [1, 3, 1, 0xa5, 0x7f, ...Array(9).fill(0)]);
  const start = service.prepare(modes, C.accumulator(S.AccumulatorAction.Start));
  assert.equal(logical(start, K.Accumulator)[0], 1);
  const stop = service.prepare(modes, C.accumulator(S.AccumulatorAction.StopAndRead));
  assert.equal(logical(stop, K.Accumulator)[0], 0);
  const values = decode(stop, K.Accumulator, [0, 0x12, 0x34, 0xab, 0xcd, 0xff, 0xfe]);
  assert.deepEqual(values, { kind: S.ReplyKind.Accumulated, values: [0x1234, 0xabcd, 0xfffe] });
  assert.ok(Object.isFrozen(values.values));
  const report = service.prepare(modes, C.peripheralReport());
  assert.deepEqual(decode(report, K.PeripheralReport, [1, 2, 3, 4, 5, 6, 7, 8]), {
    kind: S.ReplyKind.PeripheralReport,
    data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
  });
});

await test('media commands use checked packets, canonical sound choices and typed charge results', () => {
  const a = usb.app80,
    C = a.Command,
    K = catalog.service.Codec;
  const layouts = [
    ['display_option', 4],
    ['display_mode', 1],
    ['sound', 3],
    ['hdmi', 19],
    ['parameter_service', 1],
    ['charge_control', 3],
    ['charge_status', 2],
  ] as const;
  const definition = serviceDefinition(
    layouts.map(([name, count], n) => [
      210 + n,
      name,
      Array.from({ length: count }, (_, i) => (i * 5 + 7) % 32),
      4,
    ]),
  );
  const service = new a.Service(definition),
    modes = serviceModes();
  const slots = (codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec]) => {
    const entry = definition.entries.find((e) => e.codec === codec);
    assert.ok(entry);
    return entry.slots;
  };
  const prepare = (command: import('../pkg/fujimod.js').App80Command) =>
    service.prepare(modes, command);
  const logical = (
    invocation: import('../dist/index.js').App80Invocation,
    codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec],
  ) => [...slots(codec)].map((slot) => invocation.raw[1 + slot]);
  const decode = (
    invocation: import('../dist/index.js').App80Invocation,
    codec: (typeof catalog.service.Codec)[keyof typeof catalog.service.Codec],
    index: number,
    value: number,
  ) => {
    const raw = invocation.raw;
    raw[1 + slots(codec)[index]] = value;
    const reply = invocation.decode(raw);
    assert.deepEqual(reply.raw, raw);
    return reply.value;
  };
  for (const [command, codec, expected] of [
    [C.displayOption(a.DisplayOptionAction.idlePins()), K.DisplayOption, [0, 0, 0, 0]],
    [C.displayOption(a.DisplayOptionAction.forwardPair(7, 8)), K.DisplayOption, [0, 1, 7, 8]],
    [C.displayMode(a.DisplayMode.Option0), K.DisplayMode, [0]],
    [C.displayMode(a.DisplayMode.Option1), K.DisplayMode, [1]],
    [C.parameterService(a.Toggle.Disabled), K.ParameterService, [0]],
    [C.parameterService(a.Toggle.Enabled), K.ParameterService, [1]],
  ] as const) {
    const invocation = prepare(command);
    assert.deepEqual(logical(invocation, codec), expected);
    assert.deepEqual(invocation.decode(invocation.raw).value, { kind: a.ReplyKind.NoPayload });
  }
  assert.throws(
    () => new a.SoundClip(23),
    (e) =>
      a.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.argument === a.ArgumentKind.SoundClip,
  );
  for (const [format, wire] of [
    [a.SoundFormat.Format1, 0],
    [a.SoundFormat.Format2, 1],
    [a.SoundFormat.Format3, 2],
  ] as const) {
    for (const clip of [0, 22] as const)
      assert.deepEqual(
        logical(prepare(C.sound(a.SoundAction.clip(new a.SoundClip(clip), format))), K.Sound),
        [1, clip, wire],
      );
    for (const [track, base] of [
      [a.SoundTrack.Track2, 23],
      [a.SoundTrack.Track3, 26],
      [a.SoundTrack.Track4, 29],
    ] as const) {
      for (const [operation, offset] of [
        [a.SoundOperation.First, 0],
        [a.SoundOperation.Second, 1],
        [a.SoundOperation.Third, 2],
      ] as const)
        assert.deepEqual(
          logical(prepare(C.sound(a.SoundAction.track(track, operation, format))), K.Sound),
          [1, base + offset, wire],
        );
    }
  }
  assert.deepEqual(logical(prepare(C.sound(a.SoundAction.noop())), K.Sound), [0, 0, 0]);
  for (const size of [0, 15, 17] as const)
    assert.throws(
      () => new a.HdmiPacket(new Uint8Array(size)),
      (e) =>
        a.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === a.ArgumentKind.HdmiPacketLength,
    );
  assert.throws(
    () => new a.HdmiValue(4096),
    (e) =>
      a.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.argument === a.ArgumentKind.HdmiValue,
  );
  const original = Uint8Array.from({ length: 16 }, (_, n) => n),
    packet = new a.HdmiPacket(original);
  original.fill(255);
  assert.deepEqual(
    packet.data,
    Uint8Array.from({ length: 16 }, (_, n) => n),
  );
  assert.deepEqual(logical(prepare(C.hdmi(a.HdmiAction.packet(packet))), K.Hdmi), [
    0,
    1,
    0,
    ...Array.from({ length: 16 }, (_, n) => 15 - n),
  ]);
  assert.deepEqual(
    logical(
      prepare(
        C.hdmi(
          a.HdmiAction.triplet(
            new a.HdmiValue(0x123),
            new a.HdmiValue(0x456),
            new a.HdmiValue(4095),
          ),
        ),
      ),
      K.Hdmi,
    ),
    [1, 0, 0, 1, 0x23, 4, 0x56, 15, 255, ...Array(10).fill(0)],
  );
  assert.deepEqual(logical(prepare(C.hdmi(a.HdmiAction.resetStub())), K.Hdmi), Array(19).fill(0));
  for (const [action, expected] of [
    [a.ChargeAction.control(a.Toggle.Disabled), [0, 0, 255]],
    [a.ChargeAction.control(a.Toggle.Enabled), [0, 1, 255]],
    [a.ChargeAction.enableAndSetFlag(a.Toggle.Disabled), [1, 0, 255]],
    [a.ChargeAction.enableAndSetFlag(a.Toggle.Enabled), [1, 1, 255]],
  ] as const) {
    const invocation = prepare(C.chargeControl(action));
    assert.deepEqual(logical(invocation, K.ChargeControl), expected);
    for (const [raw, kind] of [
      [0, a.ChargeResultKind.Success],
      [1, a.ChargeResultKind.Failed],
      [42, a.ChargeResultKind.Unknown],
      [255, a.ChargeResultKind.Unknown],
    ] as const)
      assert.deepEqual(decode(invocation, K.ChargeControl, 2, raw), {
        kind: a.ReplyKind.ChargeApplied,
        status: kind === a.ChargeResultKind.Unknown ? { kind, rawStatus: raw } : { kind },
      });
  }
  const presence = prepare(C.chargeStatus(a.ChargeQuery.Presence));
  assert.deepEqual(logical(presence, K.ChargeStatus), [0, 255]);
  for (const [raw, kind] of [
    [0, a.ChargePresenceKind.Failed],
    [1, a.ChargePresenceKind.Absent],
    [2, a.ChargePresenceKind.Present],
    [3, a.ChargePresenceKind.Unknown],
  ] as const)
    assert.deepEqual(decode(presence, K.ChargeStatus, 1, raw), {
      kind: a.ReplyKind.ChargePresence,
      status: kind === a.ChargePresenceKind.Unknown ? { kind, rawStatus: raw } : { kind },
    });
  const state = prepare(C.chargeStatus(a.ChargeQuery.State));
  assert.deepEqual(logical(state, K.ChargeStatus), [1, 255]);
  for (const [raw, kind] of [
    [0, a.ReportedChargeStateKind.Failed],
    [1, a.ReportedChargeStateKind.Unknown],
    [2, a.ReportedChargeStateKind.Unknown],
    [3, a.ReportedChargeStateKind.Value0],
    [4, a.ReportedChargeStateKind.Values2To5],
    [5, a.ReportedChargeStateKind.Value6],
    [6, a.ReportedChargeStateKind.OtherValue],
    [255, a.ReportedChargeStateKind.Unknown],
  ] as const)
    assert.deepEqual(decode(state, K.ChargeStatus, 1, raw), {
      kind: a.ReplyKind.ChargeState,
      status: kind === a.ReportedChargeStateKind.Unknown ? { kind, rawStatus: raw } : { kind },
    });
});

await test('audio and storage codecs retain checked quantities, result bytes and zero-payload commands', () => {
  const a = usb.app80,
    C = a.Command;
  const layouts = [
    ['audio_capture', 3],
    ['audio_playback', 2],
    ['erase_storage', 2],
    ['storage_status', 1],
    ['format_storage', 0],
    ['display_setup', 0],
  ] as const;
  const slots = Object.fromEntries(
    layouts.map(([codec, n]) => [codec, Array.from({ length: n }, (_, i) => (i * 5 + 7) % 32)]),
  );
  const service = new a.Service(
      serviceDefinition(layouts.map(([codec], i) => [230 + i, codec, slots[codec], 4])),
    ),
    modes = new catalog.service.ServiceModes(4);
  const prepare = (command: import('../pkg/fujimod.js').App80Command) =>
      service.prepare(modes, command),
    logical = (p: import('../dist/index.js').App80Invocation, codec: string) =>
      slots[codec].map((i) => p.raw[i + 1]);
  const decode = (
    p: import('../dist/index.js').App80Invocation,
    codec: string,
    index: number,
    value: number,
  ) => {
    const raw = p.raw;
    raw[1 + slots[codec][index]] = value;
    return p.decode(raw).value;
  };
  for (const value of [11, 255] as const)
    assert.throws(
      () => new a.AudioCount(value),
      (e) =>
        a.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === a.ArgumentKind.AudioCount,
    );
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.audioCapture(new a.AudioCount(1), true));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.audioPlayback(99));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.eraseStorage(99));
  for (const count of [0, 10] as const)
    for (const [route, wire] of [
      [a.AudioRoute.Request82, 0],
      [a.AudioRoute.Request81, 1],
    ] as const) {
      const p = prepare(C.audioCapture(new a.AudioCount(count), route));
      assert.deepEqual(logical(p, 'audio_capture'), [count, wire, 255]);
      for (const [raw, kind] of [
        [0, a.AudioResultKind.Success],
        [1, a.AudioResultKind.Failed],
        [42, a.AudioResultKind.Unknown],
        [255, a.AudioResultKind.Unknown],
      ] as const)
        assert.deepEqual(decode(p, 'audio_capture', 2, raw), {
          kind: a.ReplyKind.AudioCaptured,
          status: kind === a.AudioResultKind.Unknown ? { kind, rawStatus: raw } : { kind },
        });
    }
  for (const [mode, wire] of [
    [a.AudioPlayback.AllLanes, 0],
    [a.AudioPlayback.ZeroSecondLane, 1],
    [a.AudioPlayback.ZeroFirstLane, 2],
  ] as const) {
    const p = prepare(C.audioPlayback(mode));
    assert.deepEqual(logical(p, 'audio_playback'), [wire, 255]);
    assert.deepEqual(decode(p, 'audio_playback', 1, 1), {
      kind: a.ReplyKind.AudioPlayed,
      status: { kind: a.AudioResultKind.Failed },
    });
  }
  for (const [action, wire] of [
    [a.StorageErase.Kind1Regions, 1],
    [a.StorageErase.Kind0And2Regions, 2],
    [a.StorageErase.Kind0And1And2Regions, 3],
    [a.StorageErase.AuxiliaryRegion, 4],
    [a.StorageErase.ResetProfiles, 5],
  ] as const) {
    const p = prepare(C.eraseStorage(action));
    assert.deepEqual(logical(p, 'erase_storage'), [wire, 255]);
    assert.deepEqual(decode(p, 'erase_storage', 1, 1), {
      kind: a.ReplyKind.StorageErased,
      status: { kind: a.StorageEraseResultKind.Failed },
    });
    assert.deepEqual(decode(p, 'erase_storage', 1, 255), {
      kind: a.ReplyKind.StorageErased,
      status: { kind: a.StorageEraseResultKind.Unknown, rawStatus: 255 },
    });
  }
  const p = prepare(C.storageStatus());
  assert.deepEqual(logical(p, 'storage_status'), [255]);
  for (const [raw, kind] of [
    [0, a.StorageStatusKind.Zero],
    [4, a.StorageStatusKind.Value4],
    [8, a.StorageStatusKind.Value8],
    [16, a.StorageStatusKind.Value16],
    [32, a.StorageStatusKind.Value32],
    [1, a.StorageStatusKind.Unknown],
    [255, a.StorageStatusKind.Unknown],
  ] as const)
    assert.deepEqual(decode(p, 'storage_status', 0, raw), {
      kind: a.ReplyKind.StorageStatus,
      status: kind === a.StorageStatusKind.Unknown ? { kind, rawStatus: raw } : { kind },
    });
  for (const command of [C.formatStorage(), C.displaySetup()] as const) {
    const invocation = prepare(command);
    assert.deepEqual(invocation.raw.slice(1), new Uint8Array(32));
    assert.deepEqual(invocation.decode(invocation.raw).value, { kind: a.ReplyKind.NoPayload });
  }
});

await test('radio and NAND service requests preserve field types and opaque snapshots', () => {
  const a = usb.app80,
    C = a.Command;
  const layouts = [
    ['nand_read_blocks', 3],
    ['option41', 1],
    ['wlan_control', 1],
    ['radio_control', 4],
    ['signal_strength', 7],
    ['iperf', 1],
  ] as const;
  const slots = Object.fromEntries(
    layouts.map(([name, count]) => [
      name,
      Array.from({ length: count }, (_, n) => (n * 5 + 7) % 32),
    ]),
  );
  const service = new a.Service(
      serviceDefinition(layouts.map(([name], n) => [200 + n, name, slots[name], 4])),
    ),
    modes = serviceModes();
  const prepare = (command: import('../pkg/fujimod.js').App80Command) =>
      service.prepare(modes, command),
    logical = (p: import('../dist/index.js').App80Invocation, name: string) =>
      slots[name].map((i) => p.raw[1 + i]);
  const decode = (
    p: import('../dist/index.js').App80Invocation,
    name: string,
    index: number,
    value: number,
  ) => {
    const raw = p.raw;
    raw[1 + slots[name][index]] = value;
    return p.decode(raw).value;
  };
  for (const [create, kind] of [
    [() => new a.RadioChannel(15), a.ArgumentKind.RadioChannel],
    [() => new a.RadioPower(128), a.ArgumentKind.RadioPower],
    [() => new a.PeerAddress(new Uint8Array(5)), a.ArgumentKind.PeerAddressLength],
  ] as const)
    assert.throws(
      create,
      (e) =>
        a.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === kind,
    );
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.nandReadBlocks(true, 0));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => a.RadioAction.secondary(99, new a.RadioChannel(1)));
  for (const [mode, m] of [
    [a.RadioMode.Mode0, 0],
    [a.RadioMode.Mode1, 1],
  ] as const) {
    for (const channel of [0, 14] as const)
      for (const power of [0, 11, 12, 127] as const)
        for (const [option, bit] of [
          [a.Toggle.Disabled, 0],
          [a.Toggle.Enabled, 128],
        ] as const) {
          const p = prepare(
            C.radioControl(
              a.RadioAction.primary(
                mode,
                new a.RadioChannel(channel),
                new a.RadioPower(power),
                option,
              ),
            ),
          );
          assert.equal(p.raw[0], 203);
          assert.deepEqual(logical(p, 'radio_control'), [m, channel, power | bit, 0]);
          assert.deepEqual(p.decode(p.raw).value, { kind: a.ReplyKind.NoPayload });
        }
    assert.deepEqual(
      logical(
        prepare(C.radioControl(a.RadioAction.secondary(mode, new a.RadioChannel(14)))),
        'radio_control',
      ),
      [m, 14, 0, 1],
    );
  }
  for (const [enabled, wire] of [
    [a.Toggle.Disabled, 0],
    [a.Toggle.Enabled, 1],
  ] as const)
    assert.deepEqual(logical(prepare(C.wlanControl(enabled)), 'wlan_control'), [wire]);
  for (const [spare, wire] of [
    [a.NandSpare.Omit, 0],
    [a.NandSpare.Include, 1],
  ] as const) {
    const p = prepare(C.nandReadBlocks(spare, 255));
    assert.deepEqual(logical(p, 'nand_read_blocks'), [wire, 255, 255]);
    for (const [raw, kind] of [
      [0, a.NandReadResultKind.Success],
      [1, a.NandReadResultKind.RejectedOrWaitError],
      [2, a.NandReadResultKind.OtherError],
      [42, a.NandReadResultKind.Unknown],
      [255, a.NandReadResultKind.Unknown],
    ] as const)
      assert.deepEqual(decode(p, 'nand_read_blocks', 2, raw), {
        kind: a.ReplyKind.NandRead,
        status: kind === a.NandReadResultKind.Unknown ? { kind, rawStatus: raw } : { kind },
      });
  }
  const option = prepare(C.option41());
  assert.deepEqual(logical(option, 'option41'), [255]);
  for (const [raw, kind] of [
    [0, a.Option41StateKind.NonZero],
    [1, a.Option41StateKind.Zero],
    [2, a.Option41StateKind.Unknown],
  ] as const)
    assert.deepEqual(decode(option, 'option41', 0, raw), {
      kind: a.ReplyKind.Option41,
      state: kind === a.Option41StateKind.Unknown ? { kind, rawStatus: raw } : { kind },
    });
  const original = new Uint8Array([1, 2, 3, 4, 5, 6]),
    address = new a.PeerAddress(original);
  original.fill(0);
  assert.deepEqual(address.data, new Uint8Array([1, 2, 3, 4, 5, 6]));
  const signal = prepare(C.signalStrength(address));
  assert.deepEqual(logical(signal, 'signal_strength'), [1, 2, 3, 4, 5, 6, 255]);
  const iperf = prepare(C.iperf());
  assert.deepEqual(logical(iperf, 'iperf'), [255]);
  for (const raw of [0, 1, 128, 255] as const) {
    assert.deepEqual(decode(signal, 'signal_strength', 6, raw), {
      kind: a.ReplyKind.SignalStrength,
      rawValue: raw,
    });
    assert.deepEqual(decode(iperf, 'iperf', 0, raw), { kind: a.ReplyKind.Iperf, rawResult: raw });
  }
});

await test('generated WPS and station values preserve snapshots and exact peer selection', () => {
  const a = usb.app80,
    C = a.Command,
    layouts = [
      ['wps', 8],
      ['station', 4],
    ] as const;
  const slots = Object.fromEntries(
    layouts.map(([name, count]) => [
      name,
      Array.from({ length: count }, (_, n) => (n * 5 + 7) % 32),
    ]),
  );
  const service = new a.Service(
      serviceDefinition(layouts.map(([name], n) => [180 + n, name, slots[name], 4])),
    ),
    modes = serviceModes();
  const prepare = (c: import('../pkg/fujimod.js').App80Command) => service.prepare(modes, c),
    logical = (p: import('../dist/index.js').App80Invocation, name: string) =>
      slots[name].map((i) => p.raw[1 + i]);
  const decode = (
    p: import('../dist/index.js').App80Invocation,
    name: string,
    index: number,
    value: number,
  ) => {
    const raw = p.raw;
    raw[1 + slots[name][index]] = value;
    return p.decode(raw).value;
  };
  for (const bytes of [
    new Uint8Array(5),
    new Uint8Array(7),
    new Uint8Array([0, 1, 2, 3, 4, 5]),
  ] as const)
    assert.throws(
      () => new a.WpsPeerAddress(bytes),
      (e) =>
        a.isError(e) &&
        e.reason.kind === usb.app80.CommandFailureKind.Argument &&
        e.reason.argument === a.ArgumentKind.WpsPeerAddress,
    );
  const bytes = new Uint8Array([2, 1, 2, 3, 4, 5]),
    peer = new a.WpsPeerAddress(bytes);
  bytes.fill(0);
  assert.deepEqual(peer.data, new Uint8Array([2, 1, 2, 3, 4, 5]));
  for (const [target, expected] of [
    [a.WpsTarget.broadcast(), [1, 0, 0, 0, 0, 0, 0, 255]],
    [a.WpsTarget.peer(peer), [1, 2, 1, 2, 3, 4, 5, 255]],
  ] as const) {
    const p = prepare(C.wps(a.WpsAction.start(target)));
    assert.deepEqual(logical(p, 'wps'), expected);
    for (const [raw, kind] of [
      [0, a.WpsCallbackResultKind.Success],
      [1, a.WpsCallbackResultKind.Timeout],
      [2, a.WpsCallbackResultKind.Failed],
      [3, a.WpsCallbackResultKind.Overlap],
      [255, a.WpsCallbackResultKind.Unknown],
    ] as const)
      assert.deepEqual(decode(p, 'wps', 7, raw), {
        kind: a.ReplyKind.WpsCallback,
        snapshot: kind === a.WpsCallbackResultKind.Unknown ? { kind, rawStatus: raw } : { kind },
      });
  }
  const phases = [
    a.WpsPhase.Idle,
    a.WpsPhase.PushButton,
    a.WpsPhase.Pin,
    a.WpsPhase.CredentialsReceived,
    a.WpsPhase.Timeout,
    a.WpsPhase.Failed,
    a.WpsPhase.Overlap,
    a.WpsPhase.Error,
  ];
  const p = prepare(C.wps(a.WpsAction.publishProgress()));
  assert.deepEqual(logical(p, 'wps'), [3, 0, 0, 0, 0, 0, 0, 255]);
  for (let raw = 0; raw < 256; raw++) {
    const snapshot =
      (raw & 15) < 8 && raw >> 4 < 2
        ? {
            kind: a.WpsProgressKind.State,
            phase: phases[raw & 15],
            connection: raw >> 4 ? a.ConnectionState.Connected : a.ConnectionState.Disconnected,
          }
        : { kind: a.WpsProgressKind.Unknown, rawValue: raw };
    assert.deepEqual(decode(p, 'wps', 7, raw), { kind: a.ReplyKind.WpsProgress, snapshot });
  }
  for (const action of [a.WpsAction.stop(), a.WpsAction.connect()] as const) {
    const p = prepare(C.wps(action));
    assert.deepEqual(p.decode(p.raw).value, { kind: a.ReplyKind.NoPayload });
  }
  const stored = prepare(C.station(a.StationAction.startStored()));
  assert.deepEqual(logical(stored, 'station'), [0, 1, 255, 0]);
  for (const raw of [0, 1, 255] as const)
    assert.deepEqual(decode(stored, 'station', 2, raw), {
      kind: a.ReplyKind.StationConnection,
      rawResult: raw,
    });
  for (const [mode, wire] of [
    [a.StationScanMode.Mode0, 0],
    [a.StationScanMode.Mode1, 1],
  ] as const) {
    const p = prepare(C.station(a.StationAction.scan(mode)));
    assert.deepEqual(logical(p, 'station'), [1, wire, 255, 255]);
    const raw = p.raw;
    raw[1 + slots.station[2]] = 1;
    raw[1 + slots.station[3]] = 255;
    assert.deepEqual(p.decode(raw).value, {
      kind: a.ReplyKind.StationScan,
      rawResult: 1,
      rawCount: 255,
    });
  }
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => a.StationAction.scan(99));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => a.WpsTarget.peer(new a.PeerAddress(new Uint8Array(6))));
});

await test('AP reconfiguration and transfer controls preserve typed results and parameter layouts', () => {
  const a = usb.app80,
    C = a.Command,
    layouts = [
      ['network_reconfigure', 14],
      ['transfer_prepare', 1],
      ['transfer_select_operation', 2],
      ['transfer_ready', 1],
      ['transfer_select_channel', 2],
      ['transfer_start', 1],
    ] as const;
  const slots = Object.fromEntries(
    layouts.map(([name, count]) => [
      name,
      Array.from({ length: count }, (_, n) => (n * 5 + 7) % 32),
    ]),
  );
  const service = new a.Service(
      serviceDefinition(layouts.map(([name], n) => [180 + n, name, slots[name], 4])),
    ),
    modes = serviceModes();
  const prepare = (c: import('../pkg/fujimod.js').App80Command) => service.prepare(modes, c),
    logical = (p: import('../dist/index.js').App80Invocation, name: string) =>
      slots[name].map((i) => p.raw[1 + i]);
  const decode = (
    p: import('../dist/index.js').App80Invocation,
    name: string,
    index: number,
    value: number,
  ) => {
    const raw = p.raw;
    raw[1 + slots[name][index]] = value;
    return p.decode(raw).value;
  };
  assert.throws(
    () => new a.CountryBytes(new Uint8Array(3)),
    (e) =>
      a.isError(e) &&
      e.reason.kind === usb.app80.CommandFailureKind.Argument &&
      e.reason.argument === a.ArgumentKind.CountryLength,
  );
  const data = new Uint8Array([85, 83]),
    country = new a.CountryBytes(data);
  data.fill(0);
  assert.deepEqual(country.data, new Uint8Array([85, 83]));
  const read = prepare(C.networkReconfigure(a.NetworkReconfigureAction.readConfiguration()));
  assert.deepEqual(
    logical(read, 'network_reconfigure'),
    [1, 0, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255],
  );
  assert.deepEqual(read.decode(read.raw).value, {
    kind: a.ReplyKind.NetworkConfiguration,
    data: new Uint8Array(6).fill(255),
  });
  for (const [radio, r] of [
    [a.AccessPointRadioMode.Mode2, 0],
    [a.AccessPointRadioMode.Mode3, 1],
  ] as const)
    for (const [security, s] of [
      [a.AccessPointSecurity.Option0, 0],
      [a.AccessPointSecurity.Option2, 2],
      [a.AccessPointSecurity.Option3, 3],
    ] as const) {
      const p = prepare(
        C.networkReconfigure(
          a.NetworkReconfigureAction.apply(255, new a.RadioChannel(14), radio, security, country),
        ),
      );
      assert.deepEqual(logical(p, 'network_reconfigure'), [
        1,
        255,
        14,
        0,
        r,
        s,
        85,
        83,
        255,
        255,
        255,
        255,
        255,
        255,
      ]);
    }
  for (const [command, codec, expected, index, kind] of [
    [C.transferPrepare(), 'transfer_prepare', [255], 0, a.ReplyKind.TransferPrepared],
    [C.transferReady(), 'transfer_ready', [255], 0, a.ReplyKind.TransferReadiness],
    [C.transferStart(), 'transfer_start', [255], 0, a.ReplyKind.TransferStarted],
    ...(
      [
        [a.TransferOperation.Operation0, 0],
        [a.TransferOperation.Operation1, 1],
        [a.TransferOperation.Operation2, 2],
        [a.TransferOperation.Operation3, 3],
        [a.TransferOperation.Operation6, 255],
      ] as const
    ).map(
      ([operation, wire]) =>
        [
          C.transferSelectOperation(operation),
          'transfer_select_operation',
          [wire, 255],
          1,
          a.ReplyKind.TransferOperationSelected,
        ] as const,
    ),
    ...(
      [
        [a.TransferChannel.Operation1, 0],
        [a.TransferChannel.Operation3, 1],
        [a.TransferChannel.Operation4, 2],
      ] as const
    ).map(
      ([channel, wire]) =>
        [
          C.transferSelectChannel(channel),
          'transfer_select_channel',
          [wire, 255],
          1,
          a.ReplyKind.TransferChannelSelected,
        ] as const,
    ),
  ] as const) {
    const p = prepare(command);
    assert.deepEqual(logical(p, codec), expected);
    for (const raw of [0, 1, 2, 3, 255] as const) {
      const group =
        codec === 'transfer_prepare' ? a.TransferPrepareResultKind : a.TransferResultKind;
      const prepareResults: Partial<
        Record<
          number,
          (typeof a.TransferPrepareResultKind)[keyof typeof a.TransferPrepareResultKind]
        >
      > = {
        0: a.TransferPrepareResultKind.Success,
        2: a.TransferPrepareResultKind.PreparationError,
        3: a.TransferPrepareResultKind.Status7,
      };
      const results: Partial<
        Record<number, (typeof a.TransferResultKind)[keyof typeof a.TransferResultKind]>
      > = { 0: a.TransferResultKind.Success, 1: a.TransferResultKind.Failed };
      const k =
        codec === 'transfer_prepare'
          ? (prepareResults[raw] ?? a.TransferPrepareResultKind.Unknown)
          : (results[raw] ?? a.TransferResultKind.Unknown);
      assert.deepEqual(decode(p, codec, index, raw), {
        kind,
        result: k === group.Unknown ? { kind: k, rawStatus: raw } : { kind: k },
      });
    }
  }
});

await test('transfer payload types keep address spaces, raw snapshots and failed words distinct', () => {
  const a = usb.app80,
    C = a.Command,
    layouts = [
      ['transfer_word', 10],
      ['transfer_pair', 7],
      ['transfer_halfword', 7],
      ['transfer_option_word', 6],
      ['transfer_bitfields', 5],
      ['transfer_byte', 3],
      ['transfer_mode_byte', 3],
      ['transfer_selected_word', 4],
      ['transfer_word7', 3],
      ['transfer_word16', 3],
      ['transfer_load_test', 1],
    ] as const;
  const slots = Object.fromEntries(
    layouts.map(([name, count]) => [
      name,
      Array.from({ length: count }, (_, n) => (n * 5 + 7) % 32),
    ]),
  );
  const service = new a.Service(
      serviceDefinition(layouts.map(([name], n) => [180 + n, name, slots[name], 4])),
    ),
    modes = serviceModes();
  const prepare = (c: import('../pkg/fujimod.js').App80Command) => service.prepare(modes, c),
    logical = (p: import('../dist/index.js').App80Invocation, name: string) =>
      slots[name].map((i) => p.raw[1 + i]);
  const decode = (
    p: import('../dist/index.js').App80Invocation,
    name: string,
    updates: readonly (readonly [number, number])[],
  ) => {
    const raw = p.raw;
    for (const [index, value] of updates) raw[1 + slots[name][index]] = value;
    return p.decode(raw).value;
  };
  assert.throws(() => new a.TransferWordAddress(65536));
  assert.throws(() => new a.TransferWordAddress(1));
  assert.throws(() => new a.TransferPairFirst(512));
  assert.throws(() => new a.TransferPayloadByte(256));
  const address = new a.TransferWordAddress(65532),
    read = prepare(C.transferReadWord(address)),
    write = prepare(C.transferWriteWord(address, 0x89abcdef));
  assert.deepEqual(logical(read, 'transfer_word'), [0, 0, 0, 255, 252, 255, 255, 255, 255, 255]);
  assert.deepEqual(logical(write, 'transfer_word'), [1, 0, 0, 255, 252, 137, 171, 205, 239, 255]);
  assert.deepEqual(
    decode(read, 'transfer_word', [
      [5, 137],
      [6, 171],
      [7, 205],
      [8, 239],
      [9, 0],
    ]),
    {
      kind: a.ReplyKind.TransferWordRead,
      result: { kind: a.TransferWordReadResultKind.Value, rawValue: 0x89abcdef },
    },
  );
  assert.deepEqual(decode(read, 'transfer_word', [[9, 1]]), {
    kind: a.ReplyKind.TransferWordRead,
    result: { kind: a.TransferWordReadResultKind.Rejected },
  });
  assert.deepEqual(decode(write, 'transfer_word', [[9, 255]]), {
    kind: a.ReplyKind.TransferWordWritten,
    result: { kind: a.TransferResultKind.Unknown, rawStatus: 255 },
  });
  const pair = prepare(C.transferPair(new a.TransferPairFirst(511), 65535));
  assert.deepEqual(logical(pair, 'transfer_pair'), [1, 255, 255, 255, 255, 255, 255]);
  for (const [raw, kind] of [
    [3, a.TransferPairResultKind.CallbackError],
    [4, a.TransferPairResultKind.Rejected],
    [6, a.TransferPairResultKind.WaitError],
  ] as const)
    assert.deepEqual(decode(pair, 'transfer_pair', [[6, raw]]), {
      kind: a.ReplyKind.TransferPair,
      result: { kind },
    });
  const full = new a.TransferHalfwordAddress(0xffffffff);
  assert.equal(full.value, 0xffffffff);
  const p = prepare(C.transferReadHalfword(full));
  assert.deepEqual(logical(p, 'transfer_halfword'), [0, 255, 255, 255, 255, 255, 255]);
  assert.equal(field(p.decode(p.raw).value, 'rawValue'), 65535);
  for (let n = 0; n < 8; n++) {
    const p = prepare(
      C.transferReadOptionWord(
        [
          a.TransferOption.Option0,
          a.TransferOption.Option1,
          a.TransferOption.Option2,
          a.TransferOption.Option3,
          a.TransferOption.Option4,
          a.TransferOption.Option5,
          a.TransferOption.Option6,
          a.TransferOption.Option7,
        ][n],
      ),
    );
    assert.deepEqual(logical(p, 'transfer_option_word'), [
      n % 4,
      Math.floor(n / 4),
      255,
      255,
      255,
      255,
    ]);
    assert.equal(field(p.decode(p.raw).value, 'rawValue'), 0xffffffff);
  }
  for (const [kind, selection, mode] of [
    [a.TransferSelectedWordKind.Kind9, 2, 1],
    [a.TransferSelectedWordKind.Kind10, 2, 0],
    [a.TransferSelectedWordKind.Kind11, 1, 1],
    [a.TransferSelectedWordKind.Kind12, 1, 0],
    [a.TransferSelectedWordKind.Kind13, 0, 1],
    [a.TransferSelectedWordKind.Kind14, 0, 0],
  ] as const) {
    const p = prepare(C.transferSetSelectedWord(kind, 0x89ab));
    assert.deepEqual(logical(p, 'transfer_selected_word'), [selection, mode, 137, 171]);
  }
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.transferReadWord(new a.WordAddress(4)));
  // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
  assert.throws(() => C.transferReadHalfword(address));
});

await test('missing workflow capabilities fail before leasing the session', async () => {
  const { session, script } = await opened(closeSteps());
  const facts = JSON.stringify({
    $schema: 'https://fujimod.com/schemas/service-workflow.generated.schema.json',
    schema: 1, firmware_id: 'body-100', firmware_sha256: 'ab'.repeat(32),
    identity: { products: ['BODY'], internal_versions: ['1.0'], code_checks: [] },
    setup: null, persistence: null,
  });
  for (const method of ['armResetGuard', 'enterDebugMode', 'leaveDebugMode'] as const) {
    assert.throws(() => session[method]('{}', 'body-100', 'ab'.repeat(32)), kind(usb.ErrorKind.Workflow));
    assert.throws(() => session[method](facts, 'other-100', 'ab'.repeat(32)), kind(usb.ErrorKind.Workflow));
    assert.throws(() => session[method](facts, 'body-100', 'ab'.repeat(32)), kind(usb.ErrorKind.Workflow));
    assert.equal(session.state, usb.SessionState.Ready);
  }
  await session.close();
  assert.equal(script.remaining, 0);
});

await test('workflow USB errors retain the existing USB tag', async () => {
  const { session } = await opened([]);
  const table = JSON.stringify({
    $schema: 'https://fujimod.com/schemas/service-workflow.generated.schema.json',
    schema: 1, firmware_id: 'body-100', firmware_sha256: 'ab'.repeat(32),
    identity: { products: ['BODY'], internal_versions: ['1.0'], code_checks: [] },
    setup: { ram_alias: 0,
      configuration: { address: 4096, cached_address: 4096, size: 256, page_bytes: 16, memory_read_bytes: 256 },
      debug_offset: 0,
      guard: { offset: 1, value: 1, aliases: [4097], code_checks: [{ name: 'guard', address: 9000, size: 4, sha256: 'ab'.repeat(32) }], state_words: [],
        callback_table: { address: 8192, words: [0] }, output_state: { address: 8200, size: 1 }, payloads: [{ name: 'main', address: 8300, size: 100 }] },
    }, persistence: null,
  });
  assert.throws(() => session.enterDebugMode(table, 'body-100', 'ab'.repeat(32)), kind(usb.ErrorKind.Workflow));
  // With no scripted product reply the synthetic endpoint reports a replay
  // mismatch, which must retain its existing USB error tag.
  await assert.rejects(session.armResetGuard(table, 'body-100', 'ab'.repeat(32)), kind(usb.ErrorKind.Usb));
  session.abandon();
});
