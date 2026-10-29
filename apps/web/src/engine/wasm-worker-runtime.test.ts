import { test, vi } from 'vite-plus/test';
import { gzipSync } from 'node:zlib';
import assert from 'node:assert/strict';
import type { EmulatorManifest } from 'fujimod/emulator';
import { WasmWorkerRuntime } from './wasm-worker-runtime';
import { testCameraModule } from './wasm-test-module';
import type {
  WasmRequest,
  WasmWorkerEvent,
} from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
import { testConfig } from './test-fixtures';
import type { CameraRecord, CameraSave } from '../cameras/store';

function setup(
  options: {
    saved?: CameraSave;
    saveError?: string;
    shared?: boolean;
    preview?: boolean;
    usb?: boolean;
  } = {},
) {
  const fake = testCameraModule(options.shared),
    events: WasmWorkerEvent[] = [],
    saves: CameraSave[] = [];
  const manifest: EmulatorManifest = {
    schema: 1,
    abi: 1,
    module: '',
    wasm: '',
    construction: 'session',
    session: options.preview ? { preview: {} } : {},
    ticksPerSecond: 12000000,
    bootTicks: 0,
    output: 0,
    usb: options.usb ? { kind: 'exr2', node: 'port' } : undefined,
  };
  const runtime = new WasmWorkerRuntime(
    (event) => {
      events.push(event);
      if (event.type === 'reply' || event.type === 'error') {
        pending.get(event.serial!)?.(event);
        pending.delete(event.serial!);
      }
    },
    async () => ({ module: fake.module, manifest, identity: 'test' }),
    {
      async saved() {
        return options.saved;
      },
      async save(_id, revision, save) {
        if (options.saveError) throw Error(options.saveError);
        saves.push(save);
        return { revision: revision + 1, savedAt: 123 } as CameraRecord;
      },
    },
  );
  let serial = 0;
  const pending = new Map<number, (event: WasmWorkerEvent) => void>();
  const send = (request: WasmRequest) =>
    new Promise<WasmWorkerEvent>((resolve) => {
      const id = ++serial;
      pending.set(id, resolve);
      runtime.receive({ type: 'rpc', serial: id, request });
    });
  const config = {
    ...testConfig(),
    bundle: 'https://example.invalid/runtime/',
    camera: { id: 'camera', revision: 3, modEnabled: false },
    execution: { mode: 'replay' as const, clockSeconds2000: 0 },
  };
  return { ...fake, runtime, events, saves, send, config, manifest };
}
test('worker routes physical inputs, scene, card operations, restart, power, profile and rollback through the Camera bindings', async () => {
  const f = setup();
  const boot = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal(boot.type, 'reply');
  const requests: WasmRequest[] = [
    { action: 'press', id: 'menu', stage: 'half' },
    { action: 'release', id: 'menu' },
    { action: 'set_position', id: 'dial', position: 'A' },
    { action: 'rotate', id: 'ring', steps: -1 },
    ...(['press', 'drag', 'release', 'tap'] as const).map((phase) => ({
      action: 'touch' as const,
      id: 'touchscreen',
      phase,
      x: 100,
      y: 200,
    })),
    { action: 'scene', scene: { width: 1, height: 1, rgb: new Uint8Array([2, 3, 4]) } },
    { action: 'scene', scene: { raf: new Uint8Array([82, 65, 70]) } },
    { action: 'power', on: false },
    { action: 'power_cycle', inputs: [{ id: 'menu' }] },
    { action: 'card_state' },
    { action: 'card_tree', revision: 1 },
    { action: 'card_read', path: '/a', options: { revision: 1, maxBytes: 20 } },
    { action: 'card_insert', present: false },
    { action: 'card_format' },
    {
      action: 'restart_card',
      files: [
        { path: '/a', bytes: new Uint8Array([9, 8]) },
        { path: '/b', bytes: new Uint8Array([7]) },
      ],
    },
    { action: 'profile', limit: 5 },
    { action: 'device_states', kind: 'dmp-smaph-h' },
    { action: 'rollback', seconds: 10 },
    {
      action: 'replay',
      steps: [
        { action: 'advance', ticks: 100 },
        { action: 'press', id: 'menu' },
        { action: 'release', id: 'menu' },
      ],
    },
  ];
  for (const request of requests)
    assert.equal((await f.send(request)).type, 'reply', request.action);
  assert.deepEqual(f.calls.find((c) => c.action === 'scene_raf')!.bytes, [82, 65, 70]);
  assert.equal(f.calls.find((c) => c.action === 'press')!.options.stage, 'half');
  const restart = f.calls.find((c) => c.action === 'restart_card')!;
  assert.deepEqual(restart.options.files, [
    { path: '/a', offset: 0, length: 2 },
    { path: '/b', offset: 2, length: 1 },
  ]);
  assert.deepEqual(restart.bytes, [9, 8, 7]);
  assert.equal(f.calls.find((c) => c.action === 'card_read')!.options.revision, 1);
  assert.equal(f.calls.find((c) => c.action === 'device_states')!.options.kind, 'dmp-smaph-h');
  assert(
    f.events.some((e) => e.type === 'metrics' && e.metrics.runtime?.ticksPerSecond === 12000000),
  );
});
test('camera saves commit portable durable bytes with optimistic revisions', async () => {
  const f = setup();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal((await f.send({ action: 'save' })).type, 'reply');
  assert.equal((await f.send({ action: 'save' })).type, 'reply');
  assert.deepEqual([...new Uint8Array(f.saves[0]!.durable)], [1, 2]);
  assert.deepEqual(
    f.events.filter((e) => e.type === 'saved').map((e) => e.revision),
    [4, 5],
  );
});
test('quota rejection is an operation error, and guest execution failure supports rollback', async () => {
  const f = setup({ saveError: 'Quota exceeded' });
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  const failedSave = await f.send({ action: 'save' });
  assert(failedSave.type === 'error' && !failedSave.fault);
  assert(f.events.some((e) => e.type === 'saved' && e.error?.includes('Quota exceeded')));
  f.fail();
  const failure = await f.send({ action: 'replay', steps: [{ action: 'advance', ticks: 20 }] });
  assert(failure.type === 'error' && failure.fault?.kind === 'guest-fault');
  assert.equal((await f.send({ action: 'rollback', seconds: 10 })).type, 'reply');
  assert.equal(
    (await f.send({ action: 'replay', steps: [{ action: 'advance', ticks: 20 }] })).type,
    'reply',
  );
});
test('shared frame leases block queued guest mutations until presentation releases the view', async () => {
  const f = setup({ shared: true });
  f.frame();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  const frame = f.events.find((event) => event.type === 'frame_view');
  assert(frame?.type === 'frame_view');
  const press = f.send({ action: 'press', id: 'menu' });
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(
    f.calls.some((call) => call.action === 'press'),
    false,
  );
  f.runtime.receive({ type: 'frame_release', lease: frame.lease + 1 });
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(
    f.calls.some((call) => call.action === 'press'),
    false,
  );
  f.runtime.receive({ type: 'frame_release', lease: frame.lease });
  assert.equal((await press).type, 'reply');
});

test('creation refusal diagnostics never ask a nonexistent board for status or frames', async () => {
  const f = setup();
  f.createError('device requires unavailable adapter');
  const boot = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert(boot.type === 'error' && boot.fault?.kind === 'unsupported-device');
  const reply = await f.send({ action: 'error_report' });
  assert(reply.type === 'reply');
  const report = reply.result as { report: { created: boolean; status: unknown; frame: unknown } };
  assert.equal(report.report.created, false);
  assert.equal(report.report.status, null);
  assert.equal(report.report.frame, null);
  assert.equal(
    f.events.some(
      (event) => event.type === 'metrics' || event.type === 'frame' || event.type === 'frame_view',
    ),
    false,
  );
});

// History uses owner-bound native checkpoints and opaque CBOR, never host RAM snapshots.
test('worker preserves checkpoint IDs and replay bytes across the transport', async () => {
  const f = setup();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  await f.send({ action: 'replay', steps: [{ action: 'advance', ticks: 12000000 }] });
  const point = await f.send({ action: 'checkpoint' });
  assert(point.type === 'reply');
  assert.deepEqual(point.result, { id: '12000000', tick: '12000000', instructions: ['24000000'] });
  await f.send({ action: 'restore_checkpoint', id: '0' });
  assert.equal(f.calls.find((c) => c.action === 'restore')!.options.id, '0');
  const log = await f.send({ action: 'history' });
  assert(log.type === 'reply');
  assert.deepEqual([...(log.result as { log: Uint8Array }).log], [0x81, 1]);
  await f.send({ action: 'replay_history', bytes: new Uint8Array([0x81, 1]) });
  assert.deepEqual(f.calls.find((c) => c.action === 'replay')!.bytes, [0x81, 1]);
});

test('declared preview policy is applied once and source metrics use observational counters', async () => {
  const f = setup({ preview: true });
  const boot = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal(boot.type, 'reply');
  assert.equal(f.calls.filter((c) => c.action === 'preview').length, 1);
  assert.equal(f.calls.filter((c) => c.action === 'preview_state').length, 1);
  assert.equal(f.calls.filter((c) => c.action === 'error_report').length, 0);
  assert(f.events.some((e) => e.type === 'metrics' && e.metrics.runtime?.sourceFrames === 0));
});

test('a portable camera save restores media before boot', async () => {
  const f = setup({
    saved: {
      durable: new Uint8Array([7, 8, 9]).buffer,
      runtimeKey: 'prior-runtime',
      positions: {},
      poweredOn: true,
      modEnabled: false,
    },
  });
  const boot = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal(boot.type, 'reply');
  assert.deepEqual(f.calls.find((c) => c.action === 'restore_durable')!.bytes, [7, 8, 9]);
  assert.equal(f.saves.length, 0);
});

test('replay advance returns while power is OFF and does not run or spin', async () => {
  const f = setup();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  await f.send({ action: 'power', on: false });
  const before = f.calls.length;
  assert.equal(
    (await f.send({ action: 'replay', steps: [{ action: 'advance', ticks: 100 }] })).type,
    'reply',
  );
  assert.equal(f.state.poweredOn, false);
  assert.equal(f.calls.length, before);
});

test('an idempotent power request does not cold boot or advance the guest', async () => {
  const f = setup();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  const before = f.calls.length;
  assert.equal((await f.send({ action: 'power', on: true })).type, 'reply');
  assert.equal(f.calls.length, before);
});

test('USB worker preserves opaque packets, advances only on usb_step, and refuses rewind', async () => {
  const f = setup({ usb: true });
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal(
    f.calls.some((call) => call.action === 'checkpoint'),
    false,
  );
  const requests: WasmRequest[] = [
    { action: 'usb_reset', node: 'port' },
    { action: 'usb_setup', node: 'port', bytes: Uint8Array.of(0x43, 0, 0, 0, 0, 0, 2, 0) },
    { action: 'usb_out', node: 'port', endpoint: 1, bytes: Uint8Array.of(0xde, 0xad) },
    { action: 'usb_in_token', node: 'port', endpoint: 0x82 },
    { action: 'usb_in', node: 'port', endpoint: 0x82, size: 512 },
    { action: 'usb_status', node: 'port' },
  ];
  const ticks = f.ticks;
  const initialTicks = ticks();
  const before = f.calls.length;
  for (const request of requests) assert.equal((await f.send(request)).type, 'reply');
  assert.deepEqual(
    f.calls.slice(before),
    requests.map(({ action, ...request }) => {
      const { bytes, ...options } = request as {
        node: string;
        endpoint?: number;
        size?: number;
        bytes?: Uint8Array;
      };
      return { action, options, bytes: [...(bytes ?? [])] };
    }),
  );
  assert.equal(ticks(), initialTicks);
  assert.equal((await f.send({ action: 'usb_step', node: 'port' })).type, 'reply');
  assert.equal(ticks(), initialTicks + 12000n);
  const undo = await f.send({ action: 'undo' });
  assert.equal(undo.type, 'error');
  if (undo.type === 'error') assert.match(undo.error, /cannot rewind/);
});

test('live cameras forward browser languages and local wall time', async () => {
  const f = setup();
  f.state.poweredOn = false;
  f.manifest.construction = 'prepared';
  f.manifest.prepared = { document: 'document.json', bootRom: 'boot.bin' };
  let supplied: { clockSeconds2000: number; preferredLanguages: string[] } | undefined;
  f.prepared((request) => {
    supplied = JSON.parse(request) as typeof supplied;
  });
  vi.stubGlobal('navigator', { languages: ['de-AT', 'en-US'] });
  vi.stubGlobal('fetch', async () => new Response('{}'));
  const localSeconds = () => {
    const date = new Date();
    return Math.floor(
      (date.valueOf() - date.getTimezoneOffset() * 60_000 - Date.UTC(2000, 0, 1)) / 1000,
    );
  };
  const before = localSeconds();
  try {
    const boot = await f.send({
      action: 'boot',
      config: {
        ...f.config,
        execution: { mode: 'live' },
      },
      dat: new ArrayBuffer(1),
      files: [],
      seedIdentity: [],
    });
    assert.equal(boot.type, 'reply', JSON.stringify(boot));
    assert.ok(supplied);
    assert.deepEqual(supplied.preferredLanguages, ['de-AT', 'en-US']);
    assert.ok(supplied.clockSeconds2000 >= before && supplied.clockSeconds2000 <= localSeconds());
  } finally {
    vi.unstubAllGlobals();
  }
});

test('prepared cameras expose controls, card storage, preview and replay through the Camera bindings', async () => {
  const f = setup();
  f.manifest.construction = 'prepared';
  f.manifest.prepared = Object.assign(
    { document: 'document.json', bootRom: 'boot.bin' },
    { nand: 'nand.gz', settings: 'settings.bin', camera: { configuration: 'camera.zst' } },
  );
  let creates = 0;
  f.prepared((text, camera) => {
    const request = JSON.parse(text) as {
      document: string;
      clockSeconds2000: number;
      preferredLanguages: string[];
      camera: [string, number][];
    };
    assert.equal(request.document, '{"number":1.0}');
    assert.equal(request.clockSeconds2000, 0);
    assert.deepEqual(request.preferredLanguages, []);
    // Camera sections arrive unchanged, packed in one span in request order.
    assert.deepEqual(request.camera, [['configuration', 14]]);
    assert.equal(new TextDecoder().decode(camera), '{"number":1.0}');
    creates++;
  });
  vi.stubGlobal(
    'fetch',
    async (url: URL) =>
      new Response(
        url.pathname.endsWith('.gz')
          ? new Uint8Array(gzipSync(Uint8Array.of(255)))
          : '{"number":1.0}',
      ),
  );
  try {
    const boot = await f.send({
      action: 'boot',
      config: f.config,
      dat: new ArrayBuffer(1),
      files: [],
      seedIdentity: [],
    });
    assert.equal(boot.type, 'reply', JSON.stringify(boot));
    assert.equal(creates, 1);
    assert.ok(f.calls.some((call) => call.action === 'state'));
    assert.ok(f.calls.some((call) => call.action === 'preview'));
    assert.ok(f.calls.some((call) => call.action === 'boot_to_rec'));
    assert.ok(f.calls.some((call) => call.action === 'preview_state'));
    assert.equal((await f.send({ action: 'card_tree' })).type, 'reply');
    assert.equal(
      (
        await f.send({
          action: 'replay',
          steps: [
            { action: 'press', id: 'menu' },
            { action: 'advance', ticks: 100 },
            { action: 'release', id: 'menu' },
          ],
        })
      ).type,
      'reply',
    );
  } finally {
    vi.unstubAllGlobals();
  }
});

test('portable snapshots load into a fresh worker without a cold boot and report bytes survive transport', async () => {
  const f = setup();
  const loaded = await f.send({
    action: 'load_snapshot',
    bytes: new Uint8Array([7, 8, 9]),
    config: f.config,
  });
  assert.equal(loaded.type, 'reply');
  assert.deepEqual(f.calls.find((c) => c.action === 'load_snapshot')?.bytes, [7, 8, 9]);
  assert.equal(
    f.calls.some((c) => ['boot_to_rec', 'restore_durable', 'power_cycle'].includes(c.action)),
    false,
  );
  const profile = await f.send({ action: 'profile' });
  assert.equal(profile.type, 'reply');
  if (profile.type === 'reply') assert.equal((profile.result as { ticks: number }).ticks, 17);
  const exported = await f.send({ action: 'export_snapshot' });
  assert.equal(exported.type, 'reply');
  if (exported.type === 'reply') assert.deepEqual(exported.result, new Uint8Array([0xa1, 0x01]));
  const report = await f.send({ action: 'error_report' });
  assert.equal(report.type, 'reply');
  if (report.type === 'reply') {
    const result = report.result as { snapshot: ArrayBuffer };
    assert.deepEqual(new Uint8Array(result.snapshot), new Uint8Array([0xa1, 0x01]));
  }
});

test('RAF uploads are parsed once and not retained for cold boot or power cycles', async () => {
  const f = setup();
  const scene = { raf: new Uint8Array([82, 65, 70]) };
  const boot = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
    scene,
  });
  assert.equal(boot.type, 'reply');
  assert.equal(f.calls.filter((call) => call.action === 'scene_raf').length, 1);
  // Later power cycles use the native owner's installed immutable frame.
  scene.raf.fill(0);
  assert.equal((await f.send({ action: 'power_cycle', inputs: [] })).type, 'reply');
  assert.equal((await f.send({ action: 'power', on: false })).type, 'reply');
  assert.equal((await f.send({ action: 'power', on: true })).type, 'reply');
  assert.equal(f.calls.filter((call) => call.action === 'scene_raf').length, 1);
  assert.deepEqual(f.calls.find((call) => call.action === 'scene_raf')!.bytes, [82, 65, 70]);
  assert.equal(
    (await f.send({ action: 'scene', scene: { raf: new Uint8Array([1, 2]) } })).type,
    'reply',
  );
  assert.equal((await f.send({ action: 'power_cycle', inputs: [] })).type, 'reply');
  assert.equal(f.calls.filter((call) => call.action === 'scene_raf').length, 2);
});

test('boot reports guest ticks against the fixed boot run', async () => {
  const f = setup();
  f.manifest.bootTicks = 3000000;
  const result = await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  assert.equal(result.type, 'reply');
  const boot = f.events.flatMap((event) =>
    event.type === 'progress'
      ? (event.assets ?? []).filter((asset) => asset.url === 'step:boot')
      : [],
  );
  assert.deepEqual(
    boot.filter((asset) => asset.status === 'loading').map((asset) => asset.loaded),
    [0, 3000000],
  );
  assert(
    boot
      .filter((asset) => asset.status === 'loading')
      .every((asset) => asset.unit === 'ticks' && asset.total === 3000000),
  );
  assert.equal(boot.at(-1)?.status, 'complete');
  const metrics = f.events.filter((event) => event.type === 'metrics').at(-1);
  assert(metrics?.type === 'metrics');
  assert.equal(metrics.metrics.guestTicks, 3000000);
});

test('a boot without a fixed tick run keeps boot progress indeterminate', async () => {
  const f = setup();
  await f.send({
    action: 'boot',
    config: f.config,
    dat: new ArrayBuffer(1),
    files: [],
    seedIdentity: [],
  });
  const boot = f.events.flatMap((event) =>
    event.type === 'progress'
      ? (event.assets ?? []).filter(
          (asset) => asset.url === 'step:boot' && asset.status === 'loading',
        )
      : [],
  );
  assert(boot.length > 0);
  assert(boot.every((asset) => asset.total === undefined));
});

test('boot holds release through native contacts when the ordinary scanner is unavailable', async () => {
  const f = setup();
  const release = vi.spyOn(f.module.Camera.prototype, 'release').mockImplementation(() => {
    throw Error('physical contact scanner is not ready: menu');
  });
  try {
    const boot = await f.send({
      action: 'boot',
      config: { ...f.config, bootInputs: [{ id: 'menu' }] },
      dat: new ArrayBuffer(1),
      files: [],
      seedIdentity: [],
    });
    assert.equal(boot.type, 'reply');
    assert.deepEqual(f.calls.find((c) => c.action === 'release_boot_inputs')?.options.inputs, [
      { id: 'menu' },
    ]);
    assert.equal(release.mock.calls.length, 0);
  } finally {
    release.mockRestore();
  }
});
