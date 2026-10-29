import { readFile } from 'node:fs/promises';
import { beforeAll, expect, test, vi } from 'vite-plus/test';
import { initialize, usb, type DeviceState } from 'fujimod';
import type { Callbacks, UsbExporter } from '../../../../crates/usb-proxy/web/types.ts';
import { installWebUsb } from '../../../../bindings/javascript/tests/webusb-fixture.ts';
import {
  createUsbCameraModel,
  type ModelDevice,
  type ModelSession,
  type ModelStream,
  type UsbCameraServices,
} from './usb-camera-model';
import { Layer, Level } from './log';
beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(
      new URL('../../../../bindings/javascript/pkg/fujimod_bg.wasm', import.meta.url),
    ),
  });
});
class Stream<T> implements ModelStream<T> {
  values: T[] = [];
  waiting?: (value: T | undefined) => void;
  ended = false;
  next(): Promise<T | undefined> {
    if (this.values.length) return Promise.resolve(this.values.shift());
    if (this.ended) return Promise.resolve(undefined);
    return new Promise((resolve) => {
      this.waiting = resolve;
    });
  }
  push(value: T) {
    if (this.waiting) {
      const resolve = this.waiting;
      this.waiting = undefined;
      resolve(value);
    } else this.values.push(value);
  }
  close() {
    this.ended = true;
    this.waiting?.(undefined);
  }
  free() {}
}
type Exchange = NonNullable<Awaited<ReturnType<ReturnType<ModelDevice['watchLog']>['next']>>>;
class CameraDevice implements ModelDevice {
  productName = 'Fixture';
  manufacturer = 'Maker';
  serialNumber = 'serial';
  status: DeviceState;
  states = new Stream<DeviceState>();
  logs = new Stream<Exchange>();
  acquire = vi.fn();
  closed = vi.fn();
  freed = vi.fn();
  releases = vi.fn();
  failClose = false;
  debugByte = 1;
  failDebugRead = false;
  debugReply?: Uint8Array;
  debugReadOverride?: () => Promise<Uint8Array>;
  onSessionClose?: (tool: string | undefined) => void;
  configurationReads = vi.fn<(...args: Parameters<ModelSession['readConfiguration']>) => void>();
  constructor(readonly id: string) {
    this.status = {
      id,
      present: true,
      open: true,
      sharing: false,
      sessionOwner: null,
      fenced: false,
    };
    this.states.push(this.status);
  }
  update(patch: Partial<DeviceState>) {
    this.status = { ...this.status, ...patch };
    this.states.push(this.status);
  }
  async open() {
    this.update({ open: true, fenced: false });
  }
  async close() {
    this.closed();
  }
  free() {
    this.freed();
  }
  async interfaces() {
    return [new usb.InterfaceSelection(1, 2, 3)];
  }
  watch() {
    return this.states;
  }
  watchLog() {
    return this.logs;
  }
  emit(kind = 0xd0, consoleData: Uint8Array | null = null, overrides: Partial<Exchange> = {}) {
    this.logs.push({
      deviceId: this.id,
      holder: this.status.sessionOwner ?? { kind: 'remote', controller: 'peer' },
      sequence: 1,
      timestampMs: 100,
      kind,
      session: 1,
      operation: 'Read memory',
      command: 0x200001,
      function: 2,
      address: 0x1234,
      selector: null,
      page: null,
      length: 4,
      requested: 4,
      transferred: 4,
      envelope: new Uint8Array(12),
      data: new Uint8Array(4),
      error: null,
      truncated: false,
      consoleData,
      ...overrides,
    });
  }
  share(): UsbExporter {
    this.update({ sharing: true });
    return {
      inventoryJson: '[]',
      execute: async () => new Uint8Array(),
      busy: () => false,
      owns: () => false,
      fenced: () => false,
      abandon: () => false,
      stopSharing: () => this.stopSharing(),
      shutdown: async () => {},
      free() {},
    };
  }
  stopSharing() {
    this.update({ sharing: false });
  }
  async acquireSession(
    _s: Parameters<ModelDevice['acquireSession']>[0],
    _l: Parameters<ModelDevice['acquireSession']>[1],
    _o: Parameters<ModelDevice['acquireSession']>[2],
    tool?: string,
  ): Promise<ModelSession> {
    this.acquire(tool);
    this.update({ sessionOwner: { kind: 'local', tool } });
    return {
      state: usb.SessionState.Ready,
      close: async () => {
        this.releases();
        this.update({ sessionOwner: null });
        this.onSessionClose?.(tool);
        if (this.failClose) throw Error('close failed');
        return new Uint8Array();
      },
      free() {},
      abandon: () => this.update({ fenced: true }),
      deviceInfo: async () => ({ text: 'Identity', raw: new Uint8Array(), free() {} }),
      configurationGeometry: async () => ({ capacity: 16, pageBytes: 4, free() {} }),
      backupFlash: async () => ({
        device: { kind: 'serial-nand', part: 'Fixture' },
        size: 4,
        sha256: 'a'.repeat(64),
      }),
      dumpState: async () => {},
      memory: async () => ({
        read: async () => Uint8Array.of(1, 2, 3, 4),
        write: async () => {},
        overwritten: null,
        close: async () => {},
        free() {},
      }),
      readConfiguration: async (...args) => {
        this.configurationReads(...args);
        if (this.failDebugRead) throw Error('configuration read failed');
        if (this.debugReadOverride) return this.debugReadOverride();
        return this.debugReply ?? Uint8Array.of(this.debugByte);
      },
      submitConsole: async (data) => {
        this.emit(0xd0, data);
        this.emit(0xd1);
        return new Uint8Array();
      },
      pollConsole: async () => ({
        progress: usb.ConsoleProgress.Pending,
        status: 0,
        output: new Uint8Array(),
        free() {},
      }),
      prepareServiceSession: async () => ({ guardBefore: 0, debugBefore: 0, progress: [] }),
      armResetGuard: async () => {
        throw Error('unused');
      },
      enterDebugMode: async () => {
        throw Error('unused');
      },
      leaveDebugMode: async () => {
        throw Error('unused');
      },
    };
  }
}
function fixture(
  devices: CameraDevice[] = [new CameraDevice('one')],
  overrides: Partial<UsbCameraServices> = {},
) {
  const callbacks = new Map<string, Callbacks>();
  let picker: ModelDevice | undefined;
  let remote: ModelDevice[] = [];
  let now = 100;
  const pause = vi.fn(async (_ms: number) => {});
  const services: Partial<UsbCameraServices> = {
    initialize: async () => {},
    discover: async () => devices,
    requestDevice: async () => picker,
    discoverProxy: async () => remote,
    now: () => now,
    pause,
    share: async (device, cb) => {
      callbacks.set(device.id, cb);
      const exporter = device.share();
      return {
        link: `https://example.com/#${device.id}`,
        host: {
          stop() {
            exporter.stopSharing();
            exporter.free();
          },
          cameraLost() {},
          cameraReturned(exporter) {
            exporter.free();
          },
        },
      };
    },
  };
  return {
    model: createUsbCameraModel({ ...services, ...overrides }),
    devices,
    callbacks,
    pause,
    picker(device: ModelDevice) {
      picker = device;
    },
    remote(device: ModelDevice) {
      remote = [device];
    },
    now(value: number) {
      now = value;
    },
  };
}
async function idle(model: ReturnType<typeof createUsbCameraModel>, count = 1) {
  await vi.waitFor(() => {
    expect(model.cameras()).toHaveLength(count);
    for (const camera of model.cameras()) {
      expect(camera.session).toBeUndefined();
      expect(camera.identity).toBeDefined();
    }
  });
}

test('multiple devices: start share → local session → stop share → local session', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    const d = f.devices[0];
    expect(d.acquire).toHaveBeenCalledExactlyOnceWith('identity');
    const other = new CameraDevice('two');
    f.picker(other);
    await f.model.connectUsb();
    await idle(f.model, 2);
    await f.model.setSharing(d.id, true);
    await f.model.withSession(d.id, 'local operation', async () => {});
    await f.model.setSharing(d.id, false);
    await f.model.withSession(d.id, 'local operation', async () => {});
    expect(f.model.cameras()[0]).toMatchObject({ present: true, uncertain: false });
    expect(f.model.cameras()[0].sharing).toBeUndefined();
    expect(d.closed).not.toHaveBeenCalled();
    expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(0);
  } finally {
    f.model.dispose();
  }
});
test('held and uncertain devices refuse before acquire; other cameras remain usable', async () => {
  const f = fixture([new CameraDevice('one'), new CameraDevice('two')]);
  try {
    await idle(f.model, 2);
    const d = f.devices[0];
    for (const patch of [
      { fenced: true },
      { fenced: false, sessionOwner: { kind: 'remote' as const, controller: 'peer' } },
      { present: false },
    ]) {
      d.update(patch);
      await Promise.resolve();
      const calls = d.acquire.mock.calls.length;
      await expect(f.model.withSession(d.id, 'read', async () => {})).rejects.toThrow();
      expect(d.acquire).toHaveBeenCalledTimes(calls);
    }
    await f.model.withSession('two', 'read', async () => {});
    let release!: () => void;
    const run = f.model.withSession(
      'two',
      'hold',
      async () =>
        new Promise<void>((r) => {
          release = r;
        }),
    );
    await vi.waitFor(() => expect(release).toBeTypeOf('function'));
    await expect(f.model.withSession('two', 'second', async () => {})).rejects.toThrow(/held/);
    release();
    await run;
  } finally {
    f.model.dispose();
  }
});
test('replug preserves id, rereads identity, and resolves unplugged problem', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    const d = f.devices[0];
    await f.model.setSharing(d.id, true);
    f.now(500);
    d.update({ present: false });
    await vi.waitFor(() => expect(f.model.cameras()[0].goneAt).toBe(500));
    expect(f.model.cameras()[0].problem).toMatch(/disconnected/);
    d.update({ present: true });
    await vi.waitFor(() => expect(d.acquire).toHaveBeenCalledTimes(2));
    await idle(f.model);
    expect(f.model.cameras()[0].id).toBe(d.id);
    expect(f.model.cameras()[0].goneAt).toBeUndefined();
    expect(f.model.cameras()[0].problem).toBeUndefined();
  } finally {
    f.model.dispose();
  }
});
test('always closes and preserves primary operation failure', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    const d = f.devices[0];
    d.failClose = true;
    await expect(
      f.model.withSession(d.id, 'operation', async () => {
        throw Error('operation failed');
      }),
    ).rejects.toThrow('operation failed');
    expect(d.releases).toHaveBeenCalledTimes(2);
  } finally {
    f.model.dispose();
  }
});
test('remote callbacks update activity/problems and await owner countdown', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    const d = f.devices[0];
    await f.model.setSharing(d.id, true);
    const cb = f.callbacks.get(d.id)!;
    cb.connected!('peer', 'job');
    d.update({ sessionOwner: { kind: 'remote', controller: 'peer' } });
    cb.activity!('peer', 'Read RAM');
    cb.progress!('peer', { label: 'Bytes', done: 4, total: 16 });
    await vi.waitFor(() =>
      expect(f.model.cameras()[0].remoteActivity).toEqual({
        controller: 'peer',
        title: 'Read RAM',
        progress: { label: 'Bytes', done: 4, total: 16 },
      }),
    );
    expect(f.model.cameras()[0].sharing?.peers.get('peer')).toBe('job');
    cb.attention!('peer', 'Check camera');
    expect(f.model.cameras()[0].problem).toBe('Check camera');
    f.model.dismissProblem(d.id);
    expect(f.model.cameras()[0].problem).toBeUndefined();
    let resolve!: () => void;
    const handler = vi.fn(
      () =>
        new Promise<void>((r) => {
          resolve = r;
        }),
    );
    const unsubscribe = f.model.onOwnerCountdown(handler);
    let done = false;
    const waiting = cb.countdown!('peer', 'Look now').then(() => {
      done = true;
    });
    expect(handler).toHaveBeenCalledWith(d.id, 'Look now');
    await Promise.resolve();
    expect(done).toBe(false);
    resolve();
    await waiting;
    unsubscribe();
    await expect(cb.countdown!('peer', 'Again')).rejects.toThrow(/unavailable/);
    cb.disconnected!('peer', 'done');
    expect(f.model.cameras()[0].remoteActivity).toBeUndefined();
  } finally {
    f.model.dispose();
  }
});
test('Rust console events log local and remote text; empty poll waits', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    const d = f.devices[0];
    await f.model.withSession(d.id, 'console', async (s) => {
      await s.submitConsole(new TextEncoder().encode('help'));
      expect((await s.pollConsole()).complete).toBe(false);
    });
    d.update({ sessionOwner: { kind: 'remote', controller: 'peer' } });
    d.emit(0xd1, new TextEncoder().encode('hello\n'));
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Console)).toHaveLength(2),
    );
    expect(
      f.model
        .log()
        .filter((row) => row.layer === Layer.Console)
        .map((row) => [row.camera, row.holder, row.message]),
    ).toEqual([
      [d.id, { kind: 'local', tool: 'console' }, 'help'],
      [d.id, { kind: 'remote', controller: 'peer', reason: '' }, 'hello\n'],
    ]);
    expect(f.pause).toHaveBeenCalledExactlyOnceWith(50);
  } finally {
    f.model.dispose();
  }
});
test('remote model uses binding events and never calls USB-only sharing on removal', async () => {
  const f = fixture([]);
  try {
    const remote = new CameraDevice('remote');
    remote.stopSharing = () => {
      throw Error('USB only');
    };
    f.remote(remote);
    await f.model.connectRemote('https://example.com/share');
    await idle(f.model);
    expect(f.model.cameras()[0].transport).toBe('remote');
    await expect(f.model.setSharing(remote.id, true)).rejects.toThrow(/remote/);
    await f.model.withSession(remote.id, 'read', async () => {});
    await f.model.remove(remote.id);
    expect(f.model.cameras()).toHaveLength(0);
    expect(remote.closed).toHaveBeenCalledOnce();
    expect(remote.freed).toHaveBeenCalledOnce();
  } finally {
    f.model.dispose();
  }
});
test('production WebUSB owner keeps one open/claim through sharing and sessions', async () => {
  const wire = installWebUsb();
  wire.state.interfaceDelay = 20;
  wire.iface.alternates[0].endpoints = [
    { endpointNumber: 2, direction: 'in', type: 'bulk', packetSize: 64 },
    { endpointNumber: 1, direction: 'out', type: 'bulk', packetSize: 64 },
  ];
  wire.iface.alternate = wire.iface.alternates[0];
  const transfer = wire.device.transferOut.bind(wire.device);
  wire.device.transferOut = async (endpoint, raw) => {
    const request = new Uint8Array(raw),
      view = new DataView(request.buffer, request.byteOffset, request.byteLength);
    if (wire.state.kind === 0xd0 && view.getUint32(0, true) === 0x150000) {
      const text = 'SYNTHETIC\0',
        response = new Uint8Array(16 + text.length * 2),
        reply = new DataView(response.buffer);
      reply.setUint32(0, 0x150000, true);
      Array.from(text).forEach((ch, i) => reply.setUint16(16 + i * 2, ch.charCodeAt(0), true));
      wire.state.pending = response;
      wire.log.push(['out', endpoint, request]);
      return { status: 'ok', bytesWritten: request.length };
    }
    return transfer(endpoint, raw);
  };
  const model = createUsbCameraModel({
    share: async (device) => {
      const exporter = device.share();
      return {
        link: 'https://example.com/share',
        host: {
          stop() {
            exporter.stopSharing();
            exporter.free();
          },
          cameraLost() {},
          cameraReturned(exporter) {
            exporter.free();
          },
        },
      };
    },
  });
  try {
    await idle(model);
    const id = model.cameras()[0].id;
    model.clearLog();
    await model.setSharing(id, true);
    await model.withSession(id, 'memory', async () => {});
    await model.setSharing(id, false);
    await model.withSession(id, 'memory', async () => {});
    await vi.waitFor(() =>
      expect(model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(4),
    );
    expect(wire.log.filter(([kind]) => kind === 'open')).toHaveLength(1);
    expect(wire.log.filter(([kind]) => kind === 'claim')).toHaveLength(1);
    expect(
      wire.log.filter(([kind]) => ['release', 'close', 'refused'].includes(kind)),
    ).toHaveLength(0);
    for (const row of model.log().filter((row) => row.layer === Layer.Ff80))
      expect(row.holder).toEqual({ kind: 'local', tool: 'memory' });
    await model.remove(id);
    expect(wire.state.interfaceChanging).toBe(false);
  } finally {
    model.dispose();
    wire.restore();
  }
});

test('stopping a pending share and then restarting creates only the current attachment', async () => {
  const device = new CameraDevice('one');
  const ready: (() => void)[] = [];
  const stopped = vi.fn();
  const f = fixture([device], {
    share: async (device) => {
      const exporter = device.share();
      await new Promise<void>((resolve) => ready.push(resolve));
      return {
        link: 'https://example.com/share',
        host: {
          stop() {
            stopped();
            exporter.stopSharing();
            exporter.free();
          },
          cameraLost() {},
          cameraReturned(exporter) {
            exporter.free();
          },
        },
      };
    },
  });
  try {
    await idle(f.model);
    const first = f.model.setSharing(device.id, true);
    await f.model.setSharing(device.id, false);
    const next = f.model.setSharing(device.id, true);
    ready[0]();
    await first;
    await vi.waitFor(() => expect(ready).toHaveLength(2));
    ready[1]();
    await next;
    expect(stopped).toHaveBeenCalledOnce();
    expect(f.model.cameras()[0].sharing).toBeDefined();
    expect(f.model.cameras()[0].present).toBe(true);
  } finally {
    f.model.dispose();
  }
});

test('one command row merges request/reply totals and payloads, with errors on either phase', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    f.model.clearLog();
    const d = f.devices[0];
    d.update({ sessionOwner: { kind: 'local', tool: 'configuration' } });
    const request = {
      operation: 'Read configuration',
      address: 0x300,
      length: 256,
      data: Uint8Array.of(1, 2, 3),
      transferred: 3,
      envelope: new Uint8Array(12).fill(0xd0),
    };
    d.emit(0xd0, null, request);
    await Promise.resolve();
    expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(0);
    d.emit(0xd1, null, {
      operation: 'Reply header',
      address: 0,
      length: 2,
      data: Uint8Array.of(4, 5),
      transferred: 2,
    });
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(1),
    );
    const row = f.model.log().find((row) => row.layer === Layer.Ff80)!;
    expect(row.message).toBe('Read configuration · 0x300 · 256 B');
    expect(row.sizes).toEqual({ sent: 27, received: 2 });
    expect(row.data).toEqual(Uint8Array.of(4, 5));
    expect(row.detail).toContain('Request payload: 01 02 03');
    expect(row.holder).toEqual({ kind: 'local', tool: 'configuration' });
    expect(row.level).toBe(Level.Debug);
    d.emit(0xd0, null, { ...request, error: 'request stall' });
    d.emit(0xd0, null, request);
    d.emit(0xd1, null, { data: Uint8Array.of(9), transferred: 1, error: 'reply disconnected' });
    d.emit(0xc0, null, {
      operation: 'Open session',
      address: null,
      length: null,
      data: new Uint8Array(8),
      transferred: 8,
    });
    d.emit(0xc4, null, {
      operation: 'Close session',
      address: null,
      length: null,
      data: new Uint8Array(2),
      transferred: 2,
    });
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(5),
    );
    const rows = f.model.log().filter((row) => row.layer === Layer.Ff80);
    expect(rows[1].level).toBe(Level.Error);
    expect(rows[1].detail).toContain('request stall');
    expect(rows[1].data).toEqual(new Uint8Array());
    expect(rows[2].level).toBe(Level.Error);
    expect(rows[2].detail).toContain('reply disconnected');
    expect(rows[2].sizes).toEqual({ sent: 27, received: 1 });
    expect(rows[2].data).toEqual(Uint8Array.of(9));
    expect(rows.slice(3).map((row) => row.message)).toEqual(['Open session', 'Close session']);
    expect(rows.slice(3).map((row) => row.sizes)).toEqual([
      { sent: 12, received: 8 },
      { sent: 12, received: 2 },
    ]);
    d.emit(0xe0, null, { operation: 'Write data', data: Uint8Array.of(7, 8), transferred: 2 });
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(6),
    );
    const upload = f.model
      .log()
      .filter((row) => row.layer === Layer.Ff80)
      .at(-1)!;
    expect(upload.data).toEqual(Uint8Array.of(7, 8));
    expect(upload.sizes).toEqual({ sent: 14, received: 0 });
  } finally {
    f.model.dispose();
  }
});

test('identity log rows name all seven wire selectors and decode their returned values', async () => {
  const f = fixture();
  try {
    await idle(f.model);
    f.model.clearLog();
    const d = f.devices[0];
    const identities = [
      'Vendor',
      'Manufacturer',
      'Product',
      'InternalVersion',
      'DeviceType',
      'Serial',
      'Framework',
    ];
    const response = (text: string) => {
      const data = new Uint8Array(16 + (text.length + 1) * 2),
        view = new DataView(data.buffer);
      view.setUint32(0, 0x150000, true);
      for (let i = 0; i < text.length; i++) view.setUint16(16 + i * 2, text.charCodeAt(i), true);
      return data;
    };
    for (const [index, name] of identities.entries()) {
      const data = response(name === 'Product' ? 'X100V' : `Value ${name}`);
      d.emit(0xd0, null, {
        command: 0x150000,
        operation: 'Device info',
        selector: 1 << index,
        address: null,
        length: null,
      });
      d.emit(0xd1, null, { command: 0x150000, selector: null, data, transferred: data.length });
    }
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(7),
    );
    expect(
      f.model
        .log()
        .filter((row) => row.layer === Layer.Ff80)
        .map((row) => row.message),
    ).toEqual(
      identities.map(
        (name) => `Device info · ${name} → ${name === 'Product' ? 'X100V' : `Value ${name}`}`,
      ),
    );
    const invalid = [
      response('X').slice(0, -2),
      response('\ud800'),
      new Uint8Array(8),
      response('Failed'),
    ];
    new DataView(invalid[3].buffer).setUint32(4, 1, true);
    for (const data of invalid) {
      d.emit(0xd0, null, {
        command: 0x150000,
        operation: 'Device info',
        selector: 4,
        address: null,
        length: null,
      });
      d.emit(0xd1, null, { data, transferred: data.length });
    }
    await vi.waitFor(() =>
      expect(f.model.log().filter((row) => row.layer === Layer.Ff80)).toHaveLength(11),
    );
    expect(
      f.model
        .log()
        .filter((row) => row.layer === Layer.Ff80)
        .slice(7)
        .map((row) => row.message),
    ).toEqual(Array(4).fill('Device info · Product'));
  } finally {
    f.model.dispose();
  }
});

test('identity reads RAM byte 0xF7 in the same session; only USB with debug off implies service mode', async () => {
  const off = new CameraDevice('off');
  off.debugByte = 0;
  const on = new CameraDevice('on');
  const nonzero = new CameraDevice('nonzero');
  nonzero.debugByte = 0x80;
  const f = fixture([off, on, nonzero]);
  try {
    await idle(f.model, 3);
    expect(f.model.cameras().map((camera) => camera.debugMode)).toEqual(['off', 'on', 'on']);
    expect(f.model.cameras().map((camera) => camera.serviceMode)).toEqual([
      true,
      undefined,
      undefined,
    ]);
    for (const device of [off, on, nonzero]) {
      expect(device.acquire).toHaveBeenCalledExactlyOnceWith('identity');
      expect(device.configurationReads).toHaveBeenCalledExactlyOnceWith(
        usb.ConfigurationView.Ram,
        0,
        247,
        1,
      );
      expect(device.releases).toHaveBeenCalledOnce();
    }
    const remote = new CameraDevice('remote');
    remote.debugByte = 0;
    f.remote(remote);
    await f.model.connectRemote('https://example.com/share');
    await idle(f.model, 4);
    expect(f.model.cameras()[3]).toMatchObject({ transport: 'remote', debugMode: 'off' });
    expect(f.model.cameras()[3].serviceMode).toBeUndefined();
  } finally {
    f.model.dispose();
  }
});

test('debug changes refresh only after close and latch service mode for the current boot', async () => {
  const device = new CameraDevice('one');
  device.debugByte = 0;
  const f = fixture([device]);
  try {
    await idle(f.model);
    expect(f.model.cameras()[0].serviceMode).toBe(true);
    const value = await f.model.withSession(device.id, 'Enable debug mode', async () => {
      device.debugByte = 1;
      expect(device.configurationReads).toHaveBeenCalledTimes(1);
      expect(device.releases).toHaveBeenCalledTimes(1);
      return 'enabled';
    });
    expect(value).toBe('enabled');
    expect(f.model.cameras()[0]).toMatchObject({ debugMode: 'on', serviceMode: true });
    expect(device.acquire.mock.calls.slice(-2)).toEqual([['Enable debug mode'], ['Debug mode']]);
    expect(device.configurationReads.mock.invocationCallOrder[1]).toBeGreaterThan(
      device.releases.mock.invocationCallOrder[1],
    );
    await f.model.withSession(device.id, 'Disable debug mode', async () => {
      device.debugByte = 0;
    });
    expect(f.model.cameras()[0]).toMatchObject({ debugMode: 'off', serviceMode: true });
    expect(device.configurationReads.mock.invocationCallOrder[2]).toBeGreaterThan(
      device.releases.mock.invocationCallOrder[3],
    );
    await f.model.withSession(device.id, 'operation', async () => {});
    expect(device.configurationReads).toHaveBeenCalledTimes(3);
    device.update({ present: false });
    await vi.waitFor(() => expect(f.model.cameras()[0].present).toBe(false));
    expect(f.model.cameras()[0].serviceMode).toBeUndefined();
    expect(f.model.cameras()[0].debugMode).toBe('unknown');
    device.debugByte = 1;
    device.update({ present: true });
    await vi.waitFor(() => expect(f.model.cameras()[0].debugMode).toBe('on'));
    expect(f.model.cameras()[0].serviceMode).toBeUndefined();
  } finally {
    f.model.dispose();
  }
});

test.each(['failure', 'empty', 'long'] as const)(
  'a %s flag read leaves identity available and debug mode unknown',
  async (kind) => {
    const device = new CameraDevice('one');
    if (kind === 'failure') device.failDebugRead = true;
    else device.debugReply = new Uint8Array(kind === 'empty' ? 0 : 2);
    const f = fixture([device]);
    try {
      await idle(f.model);
      expect(f.model.cameras()[0].debugMode).toBe('unknown');
      expect(f.model.cameras()[0].serviceMode).toBeUndefined();
      expect(device.acquire).toHaveBeenCalledExactlyOnceWith('identity');
      expect(device.releases).toHaveBeenCalledOnce();
      expect(
        f.model
          .log()
          .some(
            (row) =>
              row.layer === Layer.Connection && row.message.startsWith('Debug mode read failed:'),
          ),
      ).toBe(true);
    } finally {
      f.model.dispose();
    }
  },
);

test('a failed refresh preserves known service mode and the original debug action result', async () => {
  const device = new CameraDevice('one');
  device.debugByte = 0;
  const f = fixture([device]);
  try {
    await idle(f.model);
    expect(
      await f.model.withSession(device.id, 'Enable debug mode', async () => {
        device.failDebugRead = true;
        return 'done';
      }),
    ).toBe('done');
    expect(f.model.cameras()[0]).toMatchObject({ debugMode: 'unknown', serviceMode: true });
    await expect(
      f.model.withSession(device.id, 'Disable debug mode', async () => {
        throw Error('action failed');
      }),
    ).rejects.toThrow('action failed');
    expect(device.configurationReads).toHaveBeenCalledTimes(3);
    expect(f.model.cameras()[0]).toMatchObject({ debugMode: 'unknown', serviceMode: true });
  } finally {
    f.model.dispose();
  }
});

test('a remote session delays the refresh without sending a competing USB request', async () => {
  const device = new CameraDevice('one');
  const f = fixture([device]);
  try {
    await idle(f.model);
    device.onSessionClose = (tool) => {
      if (tool === 'Enable debug mode')
        device.update({ sessionOwner: { kind: 'remote', controller: 'peer' } });
    };
    await f.model.withSession(device.id, 'Enable debug mode', async () => {});
    expect(device.configurationReads).toHaveBeenCalledTimes(1);
    expect(device.acquire.mock.calls).toEqual([['identity'], ['Enable debug mode']]);
    expect(f.model.cameras()[0].debugMode).toBe('unknown');
    device.debugByte = 0;
    device.update({ sessionOwner: null });
    await vi.waitFor(() => expect(f.model.cameras()[0].debugMode).toBe('off'));
    expect(device.configurationReads).toHaveBeenCalledTimes(2);
    expect(device.acquire.mock.calls.at(-1)).toEqual(['Debug mode']);
  } finally {
    f.model.dispose();
  }
});

test('a flag result from before unplug/replug cannot latch service mode for the new boot', async () => {
  const device = new CameraDevice('one');
  let finish!: (data: Uint8Array) => void;
  device.debugReadOverride = () =>
    device.configurationReads.mock.calls.length === 1
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : Promise.resolve(Uint8Array.of(1));
  const f = fixture([device]);
  try {
    await vi.waitFor(() => expect(device.configurationReads).toHaveBeenCalledTimes(1));
    device.update({ present: false });
    await vi.waitFor(() => expect(f.model.cameras()[0].present).toBe(false));
    device.update({ present: true });
    await vi.waitFor(() => expect(f.model.cameras()[0].present).toBe(true));
    finish(Uint8Array.of(0));
    await vi.waitFor(() => expect(device.configurationReads).toHaveBeenCalledTimes(2));
    await idle(f.model);
    expect(f.model.cameras()[0].debugMode).toBe('on');
    expect(f.model.cameras()[0].serviceMode).toBeUndefined();
  } finally {
    f.model.dispose();
  }
});

test('a remote winning the follow-up acquisition race keeps the refresh pending until release', async () => {
  const device = new CameraDevice('one');
  const f = fixture([device]);
  try {
    await idle(f.model);
    const acquire = device.acquireSession.bind(device);
    let refused = false;
    vi.spyOn(device, 'acquireSession').mockImplementation(async (...args) => {
      if (args[3] === 'Debug mode' && !refused) {
        refused = true;
        device.update({ sessionOwner: { kind: 'remote', controller: 'peer' } });
        throw Error('Camera is in use');
      }
      return acquire(...args);
    });
    await f.model.withSession(device.id, 'Disable debug mode', async () => {
      device.debugByte = 0;
    });
    expect(refused).toBe(true);
    expect(device.configurationReads).toHaveBeenCalledTimes(1);
    expect(f.model.cameras()[0].debugMode).toBe('unknown');
    device.update({ sessionOwner: null });
    await vi.waitFor(() => expect(f.model.cameras()[0].debugMode).toBe('off'));
    expect(device.configurationReads).toHaveBeenCalledTimes(2);
  } finally {
    f.model.dispose();
  }
});

// The owner reported an X100V stuck at Reading after an automatic identity attempt.
// A failure before the session callback previously left the mode undefined forever.
test.each(['interface selection', 'session acquisition'] as const)(
  '%s failure completes identity initialization with unknown debug mode',
  async (stage) => {
    const device = new CameraDevice('one');
    const failure = Error(`${stage} refused`);
    if (stage === 'interface selection') vi.spyOn(device, 'interfaces').mockRejectedValue(failure);
    else vi.spyOn(device, 'acquireSession').mockRejectedValue(failure);
    const f = fixture([device]);
    try {
      await vi.waitFor(() => expect(f.model.cameras()[0]?.problem).toBe(failure.message));
      await vi.waitFor(() => expect(f.model.cameras()[0].session).toBeUndefined());
      expect(f.model.cameras()[0].debugMode).toBe('unknown');
      expect(device.configurationReads).not.toHaveBeenCalled();
      expect(device.releases).not.toHaveBeenCalled();
    } finally {
      f.model.dispose();
    }
  },
);

test.each(['held', 'uncertain'] as const)(
  'a %s owner reports unknown while an identity read cannot start',
  async (state) => {
    const device = new CameraDevice('one');
    device.update(
      state === 'held'
        ? { sessionOwner: { kind: 'remote', controller: 'peer' } }
        : { fenced: true },
    );
    const f = fixture([device]);
    try {
      await vi.waitFor(() => expect(f.model.cameras()).toHaveLength(1));
      expect(f.model.cameras()[0].debugMode).toBe('unknown');
      expect(device.acquire).not.toHaveBeenCalled();
      expect(device.configurationReads).not.toHaveBeenCalled();
      if (state === 'held') {
        device.update({ sessionOwner: null });
        await idle(f.model);
        expect(f.model.cameras()[0].debugMode).toBe('on');
      }
    } finally {
      f.model.dispose();
    }
  },
);

test.each(['identity selector', 'session close'] as const)(
  '%s errors still settle the debug-mode state',
  async (stage) => {
    const device = new CameraDevice('one');
    if (stage === 'session close') device.failClose = true;
    else {
      const acquire = device.acquireSession.bind(device);
      vi.spyOn(device, 'acquireSession').mockImplementation(async (...args) => {
        const session = await acquire(...args);
        return {
          ...session,
          deviceInfo: async () => {
            throw Error('identity selector refused');
          },
        };
      });
      device.failDebugRead = true;
    }
    const f = fixture([device]);
    try {
      await vi.waitFor(() => expect(device.releases).toHaveBeenCalledOnce());
      await vi.waitFor(() => expect(f.model.cameras()[0].session).toBeUndefined());
      expect(device.configurationReads).toHaveBeenCalledExactlyOnceWith(
        usb.ConfigurationView.Ram,
        0,
        247,
        1,
      );
      expect(f.model.cameras()[0].debugMode).toBe(stage === 'session close' ? 'on' : 'unknown');
    } finally {
      f.model.dispose();
    }
  },
);

test('debug mode is unknown while the initial camera reply is still pending', async () => {
  const device = new CameraDevice('one');
  let finish!: (data: Uint8Array) => void;
  device.debugReadOverride = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const f = fixture([device]);
  try {
    await vi.waitFor(() => expect(device.configurationReads).toHaveBeenCalledOnce());
    expect(f.model.cameras()[0].debugMode).toBe('unknown');
    finish(Uint8Array.of(0));
    await idle(f.model);
    expect(f.model.cameras()[0]).toMatchObject({ debugMode: 'off', serviceMode: true });
  } finally {
    f.model.dispose();
  }
});

test('saved simulations use the same session, log, sharing and removal paths without a USB picker', async () => {
  const device = new CameraDevice('simulated-saved-camera'),
    dispose = vi.fn();
  const simulate = vi.fn(async () => ({ device, dispose }));
  const f = fixture([], { simulate });
  try {
    await Promise.all([
      f.model.connectSimulated!('saved-camera'),
      f.model.connectSimulated!('saved-camera'),
    ]);
    await idle(f.model);
    expect(simulate).toHaveBeenCalledExactlyOnceWith('saved-camera');
    expect(f.model.cameras()[0]).toMatchObject({
      id: device.id,
      transport: 'simulated',
      present: true,
      identity: { product: 'Identity' },
    });
    await f.model.withSession(device.id, 'Read simulated memory', async () => {});
    await f.model.setSharing(device.id, true);
    await f.model.withSession(device.id, 'Read while sharing', async () => {});
    await f.model.setSharing(device.id, false);
    await f.model.withSession(device.id, 'Read after sharing', async () => {});
    await f.model.remove(device.id);
    expect(device.closed).toHaveBeenCalledOnce();
    expect(dispose).toHaveBeenCalledOnce();
    expect(f.model.cameras()).toHaveLength(0);
  } finally {
    f.model.dispose();
  }
});

test('builds without a simulation service omit connectSimulated', () => {
  const f = fixture([], { simulate: undefined });
  try {
    expect(typeof f.model.connectSimulated).toBe('undefined');
  } finally {
    f.model.dispose();
  }
});

test('service preparation, flash chunks and state regions use the owned workflow methods', async () => {
  vi.stubGlobal('document', { baseURI: 'https://example.com/' });
  const device = new CameraDevice('workflows');
  const acquire = device.acquireSession.bind(device);
  const prepare = vi.fn<ModelSession['prepareServiceSession']>(async () => ({
    guardBefore: 0,
    debugBefore: 1,
    progress: ['prepared'],
  }));
  const backup = vi.fn<ModelSession['backupFlash']>(
    async (_id, _metadata, _image, sink, progress) => {
      sink({ offset: 4, data: Uint8Array.of(5, 6) });
      progress?.(2, 2);
      return { device: { kind: 'serial-nand', part: 'Fixture' }, size: 2, sha256: 'b'.repeat(64) };
    },
  );
  const dump = vi.fn<ModelSession['dumpState']>(async (_id, sink, progress) => {
    sink({
      name: 'dram',
      address: 4096,
      size: 2,
      constant: false,
      sha256: 'c'.repeat(64),
      data: Uint8Array.of(7, 8),
    });
    progress?.(2, 2);
  });
  vi.spyOn(device, 'acquireSession').mockImplementation(async (...args) => ({
    ...(await acquire(...args)),
    prepareServiceSession: prepare,
    backupFlash: backup,
    dumpState: dump,
  }));
  const firmware = {
    id: 'sample-1',
    body: 'sample',
    model: 'Sample',
    version: '1',
    sha256: 'a'.repeat(64),
    bytes: 4,
    download_url: 'https://example.com/update.dat',
    release_date: '',
    service_workflow: 'runtime/sample-1/service-workflow.generated.json',
    flash_reader: 'runtime/sample-1/flash-reader.json',
  };
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: URL | string) => {
      const path = String(input);
      if (path.endsWith('service-workflow.generated.json')) return new Response('{"setup":{}}');
      if (path.endsWith('flash-reader.json'))
        return Response.json({ payload: { schema: 1 }, image: 'flash-reader.bin' });
      if (path.endsWith('flash-reader.bin')) return new Response(Uint8Array.of(1, 2, 3));
      return Response.json({ schema: 1, firmwares: [firmware] });
    }),
  );
  const f = fixture([device]);
  try {
    await idle(f.model);
    const chunk = vi.fn(),
      region = vi.fn(),
      byteProgress = vi.fn();
    await f.model.withSession(device.id, 'workflows', async (session) => {
      expect(await session.prepareServiceSession('sample-1')).toEqual({
        guardBefore: 0,
        debugBefore: 1,
        progress: ['prepared'],
      });
      expect((await session.backupFlash('sample-1', chunk, byteProgress)).sha256).toBe(
        'b'.repeat(64),
      );
      await session.dumpState('sample-1', region, byteProgress);
    });
    expect(f.model.cameras()[0]).toMatchObject({ serviceMode: true, debugMode: 'on' });
    expect(prepare).toHaveBeenCalledExactlyOnceWith('{"setup":{}}', 'sample-1', 'a'.repeat(64));
    expect(backup).toHaveBeenCalledWith(
      'sample-1',
      '{"schema":1}',
      Uint8Array.of(1, 2, 3),
      chunk,
      byteProgress,
    );
    expect(dump).toHaveBeenCalledExactlyOnceWith('sample-1', region, byteProgress);
    expect(byteProgress).toHaveBeenCalledTimes(2);
    expect(byteProgress).toHaveBeenCalledWith(2, 2);
    expect(chunk).toHaveBeenCalledExactlyOnceWith({ offset: 4, data: Uint8Array.of(5, 6) });
    expect(region).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'dram', data: Uint8Array.of(7, 8) }),
    );
  } finally {
    f.model.dispose();
    vi.unstubAllGlobals();
  }
});
