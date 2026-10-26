import type { Message, Operation, Completion, Descriptor, ShareStatus } from './types.ts';
// Version 5: every message is one postcard value (PIPELINE.md): unsigned
// LEB128 varints, length-prefixed UTF-8 strings and byte strings, one-varint
// enum tags. The order of variants and fields below is the wire format and
// matches src/protocol.rs; vectors.json, generated from the Rust types, keeps
// the two in step. Payload bytes are decoded as views, never copied or texted.
export const VERSION = 5;
export const MAX_TRANSFER = 8 * 1024 * 1024;
export const MAX_MESSAGE = MAX_TRANSFER + 65536;
const MESSAGES = [
  'hello',
  'ready',
  'goodbye',
  'status',
  'attention',
  'activity',
  'progress_start',
  'progress_update',
  'progress_done',
  'inventory',
  'execute',
  'expect',
  'sync',
  'result',
  'expect_failed',
  'synced',
  'routed',
] as const;
const OPERATIONS = [
  'probe',
  'list',
  'owner_status',
  'owner_attention',
  'open',
  'close',
  'control_in',
  'control_out',
  'transfer_in',
  'transfer_out',
  'clear_halt',
] as const;
const REQUEST_TYPES = ['standard', 'class', 'vendor'];
const RECIPIENTS = ['device', 'interface', 'endpoint', 'other'];
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

class Writer {
  bytes = new Uint8Array(256);
  at = 0;
  reserve(length: number) {
    if (this.at + length <= this.bytes.length) return;
    const grown = new Uint8Array(Math.max(this.bytes.length * 2, this.at + length));
    grown.set(this.bytes.subarray(0, this.at));
    this.bytes = grown;
  }
  u8(value: number) {
    if (!Number.isInteger(value) || value < 0 || value > 255) throw Error('Invalid byte value');
    this.reserve(1);
    this.bytes[this.at++] = value;
  }
  varint(value: number) {
    if (!Number.isSafeInteger(value) || value < 0) throw Error('Invalid unsigned integer');
    this.reserve(8);
    while (value >= 128) {
      this.bytes[this.at++] = (value % 128) | 128;
      value = Math.floor(value / 128);
    }
    this.bytes[this.at++] = value;
  }
  raw(data: Uint8Array) {
    this.varint(data.length);
    this.reserve(data.length);
    this.bytes.set(data, this.at);
    this.at += data.length;
  }
  data(data: unknown) {
    if (!(data instanceof Uint8Array) || data.length > MAX_TRANSFER)
      throw Error('Invalid binary USB payload');
    this.raw(data);
  }
  string(text: unknown) {
    if (typeof text !== 'string') throw Error('Expected text');
    this.raw(encoder.encode(text));
  }
  option<T>(value: T | null | undefined, write: (value: T) => void) {
    if (value === null || value === undefined) this.u8(0);
    else {
      this.u8(1);
      write(value);
    }
  }
  sequence<T>(values: T[] | undefined, write: (value: T) => void) {
    const items = values ?? [];
    this.varint(items.length);
    for (const item of items) write(item);
  }
  tag(names: readonly string[], name: unknown) {
    const index = names.indexOf(name as string);
    if (index < 0) throw Error(`Unknown variant ${String(name)}`);
    this.varint(index);
  }
  finish() {
    return this.bytes.subarray(0, this.at);
  }
}
class Reader {
  bytes: Uint8Array;
  at = 0;
  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }
  u8() {
    if (this.at >= this.bytes.length) throw Error('Truncated proxy message');
    return this.bytes[this.at++];
  }
  varint() {
    let value = 0,
      scale = 1;
    for (let i = 0; i < 10; i++) {
      const byte = this.u8();
      value += (byte & 127) * scale;
      if (!(byte & 128)) {
        if (!Number.isSafeInteger(value)) throw Error('Integer exceeds safe range');
        return value;
      }
      scale *= 128;
    }
    throw Error('Invalid varint');
  }
  bounded(maximum: number) {
    const value = this.varint();
    if (value > maximum) throw Error('Integer out of range');
    return value;
  }
  raw() {
    const length = this.varint();
    if (length > this.bytes.length - this.at) throw Error('Truncated proxy message');
    const data = this.bytes.subarray(this.at, this.at + length);
    this.at += length;
    return data;
  }
  string() {
    return decoder.decode(this.raw());
  }
  bool() {
    const value = this.u8();
    if (value > 1) throw Error('Invalid boolean');
    return value === 1;
  }
  option<T>(read: () => T): T | null {
    return this.bool() ? read() : null;
  }
  sequence<T>(read: () => T): T[] {
    const length = this.varint();
    if (length > this.bytes.length - this.at) throw Error('Truncated proxy message');
    return Array.from({ length }, read);
  }
  tag<T extends string>(names: readonly T[]): T {
    const name = names[this.varint()];
    if (name === undefined) throw Error('Unknown variant');
    return name;
  }
}

function writeStatus(w: Writer, status: ShareStatus) {
  w.u8(status.online ? 1 : 0);
  w.option(status.reason, (v) => w.string(v));
  w.option(status.battery, (v) => w.string(v));
  w.option(status.battery_age_ms, (v) => w.varint(v));
}
function readStatus(r: Reader): ShareStatus {
  const status: ShareStatus = { online: r.bool() };
  const reason = r.option(() => r.string()),
    battery = r.option(() => r.string()),
    age = r.option(() => r.varint());
  if (reason !== null) status.reason = reason;
  if (battery !== null) status.battery = battery;
  if (age !== null) status.battery_age_ms = age;
  return status;
}
function writeDevice(w: Writer, d: Descriptor) {
  w.string(d.id);
  w.varint(d.vendor_id);
  w.varint(d.product_id);
  w.option(d.manufacturer, (v) => w.string(v));
  w.option(d.product, (v) => w.string(v));
  w.option(d.serial, (v) => w.string(v));
  w.sequence(d.interfaces, (i) => {
    w.u8(i.configuration);
    w.u8(i.interface);
    w.u8(i.alternate);
    w.sequence(i.endpoints, (e) => {
      w.u8(e.address);
      w.string(e.kind);
      w.varint(e.packet_size);
    });
  });
  w.option(d.status, (s) => writeStatus(w, s));
}
function readDevice(r: Reader): Descriptor {
  const device: Descriptor = {
    id: r.string(),
    vendor_id: r.bounded(65535),
    product_id: r.bounded(65535),
    manufacturer: r.option(() => r.string()),
    product: r.option(() => r.string()),
    serial: r.option(() => r.string()),
    interfaces: r.sequence(() => ({
      configuration: r.u8(),
      interface: r.u8(),
      alternate: r.u8(),
      endpoints: r.sequence(() => ({
        address: r.u8(),
        kind: r.string(),
        packet_size: r.bounded(65535),
      })),
    })),
  };
  const status = r.option(() => readStatus(r));
  if (status) device.status = status;
  return device;
}
function writeSetup(w: Writer, setup: Operation['setup']) {
  if (!setup) throw Error('Missing control setup');
  w.tag(REQUEST_TYPES, setup.request_type);
  w.tag(RECIPIENTS, setup.recipient);
  w.u8(setup.request);
  w.varint(setup.value ?? 0);
  w.varint(setup.index ?? 0);
}
function readSetup(r: Reader) {
  return {
    request_type: r.tag(REQUEST_TYPES),
    recipient: r.tag(RECIPIENTS),
    request: r.u8(),
    value: r.bounded(65535),
    index: r.bounded(65535),
  };
}
function writeOperation(w: Writer, op: Operation) {
  w.tag(OPERATIONS, op.op);
  switch (op.op) {
    case 'owner_status':
    case 'owner_attention':
      w.string(op.message);
      break;
    case 'open':
      w.u8(op.configuration ?? 0);
      w.u8(op.interface ?? 0);
      w.u8(op.alternate ?? 0);
      break;
    case 'control_in':
      writeSetup(w, op.setup);
      w.varint(op.length ?? 0);
      break;
    case 'control_out':
      writeSetup(w, op.setup);
      w.data(op.data);
      break;
    case 'transfer_in':
      w.u8(op.endpoint ?? 0);
      w.varint(op.length ?? 0);
      break;
    case 'transfer_out':
      w.u8(op.endpoint ?? 0);
      w.data(op.data);
      break;
    case 'clear_halt':
      w.u8(op.endpoint ?? 0);
      break;
  }
}
function readOperation(r: Reader): Operation {
  const op = r.tag(OPERATIONS);
  switch (op) {
    case 'owner_status':
    case 'owner_attention':
      return { op, message: r.string() };
    case 'open':
      return { op, configuration: r.u8(), interface: r.u8(), alternate: r.u8() };
    case 'control_in':
      return { op, setup: readSetup(r), length: r.bounded(MAX_TRANSFER) };
    case 'control_out':
      return { op, setup: readSetup(r), data: r.raw() };
    case 'transfer_in':
      return { op, endpoint: r.u8(), length: r.bounded(MAX_TRANSFER) };
    case 'transfer_out':
      return { op, endpoint: r.u8(), data: r.raw() };
    case 'clear_halt':
      return { op, endpoint: r.u8() };
    default:
      return { op };
  }
}
function writeCompletion(w: Writer, c: Completion) {
  w.sequence(c.devices, (d) => writeDevice(w, d));
  w.data(c.data ?? new Uint8Array());
  w.varint(c.transferred ?? 0);
  w.option(c.error, (e) => w.string(e));
}
function validateExpect(m: Extract<Message, { type: 'expect' }>) {
  if (m.bytes.length > MAX_TRANSFER || (m.mask != null && m.mask.length !== m.bytes.length))
    throw Error('Expectation mask must match its bytes');
  if (m.length != null && (!Number.isInteger(m.length) || m.length < 0 || m.length > MAX_TRANSFER))
    throw Error('Expectation length exceeds transfer limit');
  if ((m.alternatives?.length ?? 0) > 8 || m.alternatives?.some((a) => a.length !== m.bytes.length))
    throw Error('Expectation alternatives must have equal lengths (at most eight)');
}
function readCompletion(r: Reader): Completion {
  return {
    devices: r.sequence(() => readDevice(r)),
    data: r.raw(),
    transferred: r.varint(),
    error: r.option(() => r.string()),
  };
}
function write(w: Writer, m: Message) {
  w.tag(MESSAGES, m.type);
  switch (m.type) {
    case 'hello':
      w.varint(m.version);
      w.string(m.role);
      w.string(m.token);
      w.option(m.reason, (v) => w.string(v));
      break;
    case 'ready':
      w.varint(m.version);
      w.string(m.id);
      break;
    case 'goodbye':
      w.string(m.reason);
      break;
    case 'status':
      w.string(m.battery);
      break;
    case 'attention':
      w.string(m.message);
      break;
    case 'activity':
      w.string(m.title);
      break;
    case 'progress_start':
      w.string(m.label);
      w.varint(m.total);
      break;
    case 'progress_update':
      w.varint(m.done);
      break;
    case 'progress_done':
      break;
    case 'inventory':
      w.sequence(m.devices, (d) => writeDevice(w, d));
      break;
    case 'execute':
      w.varint(m.seq);
      w.string(m.device);
      w.varint(m.timeout_ms);
      writeOperation(w, m.op);
      break;
    case 'expect':
      validateExpect(m);
      w.varint(m.seq);
      w.varint(m.of);
      w.varint(m.offset);
      w.data(m.bytes);
      w.option(m.mask, (v) => w.data(v));
      w.option(m.length, (v) => w.varint(v));
      w.sequence(m.alternatives ?? [], (v) => w.data(v));
      break;
    case 'sync':
      w.varint(m.seq);
      break;
    case 'result':
      w.varint(m.seq);
      writeCompletion(w, m.completion);
      break;
    case 'expect_failed':
      w.varint(m.seq);
      w.data(m.actual);
      break;
    case 'synced':
      w.varint(m.seq);
      w.varint(m.discarded);
      break;
    case 'routed':
      if (m.message.type === 'routed') throw Error('Nested route');
      w.varint(m.stream);
      write(w, m.message);
      break;
  }
}
function read(r: Reader, nested = false): Message {
  const type = r.tag(MESSAGES);
  switch (type) {
    case 'hello':
      return {
        type,
        version: r.varint(),
        role: r.string(),
        token: r.string(),
        reason: r.option(() => r.string()),
      };
    case 'ready':
      return { type, version: r.varint(), id: r.string() };
    case 'goodbye':
      return { type, reason: r.string() };
    case 'status':
      return { type, battery: r.string() };
    case 'attention':
      return { type, message: r.string() };
    case 'activity':
      return { type, title: r.string() };
    case 'progress_start':
      return { type, label: r.string(), total: r.varint() };
    case 'progress_update':
      return { type, done: r.varint() };
    case 'progress_done':
      return { type };
    case 'inventory':
      return { type, devices: r.sequence(() => readDevice(r)) };
    case 'execute':
      return {
        type,
        seq: r.varint(),
        device: r.string(),
        timeout_ms: r.varint(),
        op: readOperation(r),
      };
    case 'expect': {
      const message = {
        type,
        seq: r.varint(),
        of: r.varint(),
        offset: r.varint(),
        bytes: r.raw(),
        mask: r.option(() => r.raw()),
        length: r.option(() => r.varint()),
        alternatives: r.sequence(() => r.raw()),
      };
      validateExpect(message);
      return message;
    }
    case 'sync':
      return { type, seq: r.varint() };
    case 'result':
      return { type, seq: r.varint(), completion: readCompletion(r) };
    case 'expect_failed':
      return { type, seq: r.varint(), actual: r.raw() };
    case 'synced':
      return { type, seq: r.varint(), discarded: r.varint() };
    case 'routed': {
      if (nested) throw Error('Nested route');
      return { type, stream: r.varint(), message: read(r, true) };
    }
  }
}
export function encodeMessage(message: Message) {
  const w = new Writer();
  write(w, message);
  const bytes = w.finish();
  if (bytes.length > MAX_MESSAGE) throw Error('Message exceeds limit');
  return bytes;
}
export function decodeMessage(input: unknown): Message {
  const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : input;
  if (!(bytes instanceof Uint8Array) || !bytes.length || bytes.length > MAX_MESSAGE)
    throw Error('Invalid binary proxy message');
  const r = new Reader(bytes);
  const message = read(r);
  if (r.at !== bytes.length) throw Error('Trailing bytes after proxy message');
  return message;
}
/** Whether IN result bytes `offset..` equal `bytes`, both ANDed with `mask`. */
export function expectationHolds(
  data: Uint8Array | undefined,
  offset: number,
  bytes: Uint8Array,
  mask?: Uint8Array | null,
) {
  if (!data || offset + bytes.length > data.length) return false;
  for (let i = 0; i < bytes.length; i++) {
    const m = mask ? mask[i] : 255;
    if ((data[offset + i] & m) !== (bytes[i] & m)) return false;
  }
  return true;
}
