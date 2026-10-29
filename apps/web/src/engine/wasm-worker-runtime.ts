import {
  DownloadProgress,
  bootProgress,
  loadingStep,
  type LoadingProgress,
} from '@fujimod/emulator-ui/engine/download-progress';
import type { CardFile, EngineConfig, ReplayStep, Scene } from '@fujimod/emulator-ui/engine/api';
import { loadPreparedBundle } from './prepared-bundle';
import { CameraStore } from '../cameras/store';
import { digest } from './identity';
import { diagnosticReport } from './diagnostics';
import { faultFrom } from '@fujimod/emulator-ui/engine/fault';
import {
  createCamera,
  HostTimings,
  instantiateEmulatorModule,
  isGuestExecutionError,
  parseEmulatorManifest,
  type BootInput,
  type Camera,
  type EmulatorManifest,
  type EmulatorModule,
  type EmulatorModuleFactory,
  type Point,
  type SessionState,
} from 'fujimod/emulator';
import type {
  WasmRequest,
  WasmWorkerEvent,
  WasmWorkerRequest,
} from '@fujimod/emulator-ui/engine/wasm-worker-protocol';

type Send = (event: WasmWorkerEvent, transfer?: Transferable[]) => void;
/** A retained checkpoint and its guest time. */
type Checkpoint = Point & { id: string };
interface Loaded {
  module: EmulatorModule;
  manifest: EmulatorManifest;
  identity: string;
  dmpWebGpu?: DmpWebGpuModule;
  loadTimings?: ReturnType<HostTimings['snapshot']>;
}
interface DmpWebGpuModule {
  render_request(
    request: Uint8Array,
    read_guest: (address: bigint, length: number) => Uint8Array,
  ): Promise<Uint8Array>;
  render_vector_ui_request(
    request: Uint8Array,
    read_guest: (address: bigint, length: number) => Uint8Array,
  ): Promise<Uint8Array>;
}
export async function loadWasm(
  bundle: string,
  progress: LoadingProgress = () => {},
): Promise<Loaded> {
  const timing = new HostTimings();
  const downloads = new DownloadProgress(progress);
  const response = await timing.measureAsync('manifestFetch', () =>
    downloads.fetch(new URL('wasm.json', bundle).href, () => fetch(new URL('wasm.json', bundle))),
  );
  if (!response.ok) throw Error('Wasm runtime manifest is unavailable.');
  const manifest = await timing.measureAsync('manifestParse', async () => {
    const raw: unknown = await response.json();
    return parseEmulatorManifest(raw);
  });
  // Load the independent module text and binary concurrently.
  const [wasmBinary, create] = await Promise.all([
    timing.measureAsync('binaryFetch', async () => {
      const url = new URL(manifest.wasm, new URL('wasm.json', bundle)).href;
      const response = await downloads.fetch(url, () => fetch(url));
      if (!response.ok) throw Error('Wasm binary is unavailable.');
      return response.arrayBuffer();
    }),
    timing.measureAsync('moduleImport', async () => {
      const url = new URL(manifest.module, new URL('wasm.json', bundle));
      return downloads.module(url.href, async () => {
        const imported: unknown = await import(/* @vite-ignore */ url.href);
        if (imported === null || typeof imported !== 'object' || !('default' in imported))
          throw Error('Generated module has no factory.');
        const factory = imported.default;
        if (typeof factory !== 'function') throw Error('Generated module has no factory.');
        return factory as EmulatorModuleFactory;
      });
    }),
  ]);
  const dmpCpuRenderer = manifest.dmpCpuRenderer ?? true;
  const module = await timing.measureAsync('instantiate', () =>
    instantiateEmulatorModule(
      create,
      wasmBinary,
      (path) => new URL(path, new URL(manifest.wasm, new URL('wasm.json', bundle))).href,
      {
        executionPolicy: manifest.executionPolicy,
        dmpCpuRenderer,
      },
    ),
  );
  let dmpWebGpu: DmpWebGpuModule | undefined;
  let dmpWebGpuIdentity: string | undefined;
  if (!dmpCpuRenderer) {
    const base = new URL(manifest.wasm, new URL('wasm.json', bundle));
    const moduleUrl = new URL('dmp_webgpu.js', base);
    const wasmUrl = new URL('dmp_webgpu_bg.wasm', base);
    const [imported, wasmResponse] = await Promise.all([
      timing.measureAsync('dmpWebGpuImport', async () => import(/* @vite-ignore */ moduleUrl.href)),
      timing.measureAsync('dmpWebGpuBinary', async () => fetch(wasmUrl.href)),
    ]);
    if (!wasmResponse.ok) throw Error('DMP WebGPU binary is unavailable.');
    if (
      imported === null ||
      typeof imported !== 'object' ||
      !('default' in imported) ||
      typeof imported.default !== 'function' ||
      !('render_request' in imported) ||
      typeof imported.render_request !== 'function' ||
      !('render_vector_ui_request' in imported) ||
      typeof imported.render_vector_ui_request !== 'function'
    )
      throw Error('DMP WebGPU module has no renderer exports.');
    const webgpuBinary = await wasmResponse.arrayBuffer();
    dmpWebGpuIdentity = await digest(webgpuBinary);
    await timing.measureAsync('dmpWebGpuInitialize', () =>
      (imported.default as (input: ArrayBuffer) => Promise<unknown>)(webgpuBinary),
    );
    dmpWebGpu = imported as DmpWebGpuModule;
  }
  const cameraIdentity = await timing.measureAsync('binaryDigest', () => digest(wasmBinary));
  const identity = dmpWebGpuIdentity
    ? await digest(new TextEncoder().encode(`${cameraIdentity}:${dmpWebGpuIdentity}`))
    : cameraIdentity;
  return {
    module,
    manifest,
    identity,
    dmpWebGpu,
    loadTimings: timing.snapshot(),
  };
}
function packFiles(files: CardFile[]) {
  let offset = 0;
  const rows = files.map(({ path, bytes }) => {
    const row = { path, offset, length: bytes.length };
    offset += bytes.length;
    return row;
  });
  const bytes = new Uint8Array(offset);
  files.forEach((file, i) => bytes.set(file.bytes, rows[i]!.offset));
  return { rows, bytes };
}
/** Show a scene, or restore the analytic one. */
function showScene(camera: Camera, scene: Scene | null) {
  if (scene && 'raf' in scene) camera.sceneRaf(scene.raf);
  else if (scene) camera.setScene(scene.width, scene.height, scene.rgb);
  else camera.clearScene();
}

/** One serialized owner. Rendering borrows Wasm memory before any next mutation. */
export class WasmWorkerRuntime {
  private timings = new HostTimings();
  private loadTimings?: Loaded['loadTimings'];
  private loaded?: Promise<Loaded>;
  private bundle?: string;
  private module?: EmulatorModule;
  private camera?: Camera;
  private manifest?: EmulatorManifest;
  private dmpWebGpu?: DmpWebGpuModule;
  private config?: EngineConfig;
  private state?: SessionState;
  private scene?: Scene;
  private queue: Promise<void> = Promise.resolve();
  private timer?: ReturnType<typeof setTimeout>;
  private live = false;
  private failed = false;
  private replayMode = false;
  private clockWall = 0;
  private clockTicks = 0n;
  /** Recent (wall, guest tick) samples; speed spans about two seconds so menus and live view read alike. */
  private speedSamples: { wall: number; ticks: bigint }[] = [];
  private lastPace = 0;
  private started = 0;
  private startedTicks = 0n;
  private nextSave = Infinity;
  private points: Checkpoint[] = [];
  private nextCheckpoint = 0n;
  private nextPresentation = 0n;
  private nextInspection = 0n;
  private sourceFrames?: number;
  private sourceTicks = 0n;
  private canvas?: OffscreenCanvas;
  private lease = 0;
  private release?: () => void;
  private leased?: Promise<void>;
  private runtimeKey = '';
  constructor(
    private send: Send,
    private loader = loadWasm,
    private cameras: Pick<CameraStore, 'saved' | 'save'> = new CameraStore(),
  ) {
    const transport = send;
    this.send = (event, transfer) =>
      this.timings.measure('postMessage', () => transport(event, transfer));
  }
  receive(message: WasmWorkerRequest) {
    if (message.type === 'frame_release') {
      if (message.lease === this.lease) {
        this.release?.();
        this.release = undefined;
        this.leased = undefined;
      }
      return;
    }
    this.enqueue(async () => {
      try {
        const result = await this.dispatch(message.request);
        this.send(
          { type: 'reply', serial: message.serial, result },
          result instanceof Uint8Array && result.buffer instanceof ArrayBuffer
            ? [result.buffer]
            : result !== null &&
                typeof result === 'object' &&
                'snapshot' in result &&
                result.snapshot instanceof ArrayBuffer
              ? [result.snapshot]
              : [],
        );
      } catch (error) {
        const fatal =
          message.request.action === 'boot' ||
          isGuestExecutionError(error) ||
          error instanceof WebAssembly.RuntimeError;
        if (fatal) {
          this.failed = true;
          this.stop();
        }
        this.send({
          type: 'error',
          serial: message.serial,
          error: error instanceof Error ? error.message : String(error),
          fault: fatal ? faultFrom(error) : undefined,
        });
      }
    });
  }
  private enqueue(operation: () => Promise<void>) {
    this.queue = this.queue.then(async () => {
      await this.leased;
      await operation();
    });
  }
  private prepare(bundle: string) {
    if (this.bundle && this.bundle !== bundle)
      throw Error('A worker owns exactly one Wasm runtime.');
    this.bundle = bundle;
    return (this.loaded ??= this.loader(bundle, (message, percent, assets) =>
      this.send({ type: 'progress', message, percent, assets }),
    ));
  }
  private require() {
    if (!this.camera) throw Error('Wasm session is not ready.');
    return this.camera;
  }
  private async completeDmpRequests(): Promise<boolean> {
    if (this.manifest?.dmpCpuRenderer !== false) return false;
    const camera = this.require();
    const renderer = this.dmpWebGpu;
    if (!renderer) throw Error('DMP WebGPU request has no initialized renderer.');
    const requests = camera.takeHostRequests();
    if (requests.length === 0) return false;
    try {
      for (const request of requests) {
        const readGuest = (address: bigint, length: number) => camera.memoryRead(address, length);
        let result: Uint8Array;
        if (request.kind === 'vector-ui-webgpu') {
          result = await renderer.render_vector_ui_request(request.payload, readGuest);
        } else if (['dmp-webgpu', 'vector-gles-webgpu'].includes(request.kind)) {
          result = await renderer.render_request(request.payload, readGuest);
        } else {
          throw Error(`Unsupported asynchronous device request ${request.kind}.`);
        }
        camera.completeHostRequest(request.device, request.id, result);
      }
    } finally {
      requests.forEach((request) => request.free());
    }
    return true;
  }
  private async runUntil(tick: bigint, progress?: (instructions: number) => void) {
    const camera = this.require();
    do {
      camera.runUntil(tick, progress);
    } while (await this.completeDmpRequests());
  }
  private async bootToRec(limit: bigint, progress?: (instructions: number) => void) {
    const camera = this.require();
    const initialInstructions = camera.status().instructions;
    let report: ReturnType<Camera['bootToRec']> | undefined;
    for (;;) {
      const consumed = camera.status().instructions - initialInstructions;
      if (consumed >= limit) {
        if (!report) return camera.bootToRec(1n, progress);
        report.status = 'instruction-limit';
        return report;
      }
      report = camera.bootToRec(limit - consumed, progress);
      if (!(await this.completeDmpRequests())) return report;
    }
  }
  private observe(state: SessionState) {
    this.state = state;
    this.send({ type: 'state', state });
  }
  private observeCurrent() {
    if (this.manifest && this.manifest.construction !== 'resolved')
      this.observe(this.require().state());
  }
  private async dispatch(request: WasmRequest): Promise<unknown> {
    if (request.action === 'prepare') {
      await this.prepare(request.bundle);
      return;
    }
    if (request.action === 'boot')
      return this.boot(
        request.dat,
        request.config,
        request.files,
        request.scene,
        request.seedIdentity,
      );
    if (request.action === 'load_snapshot') return this.loadSnapshot(request.bytes, request.config);
    if (request.action === 'error_report') return this.errorReport();
    const s = this.require();
    if (
      this.manifest?.usb &&
      [
        'checkpoint',
        'restore_checkpoint',
        'undo',
        'history',
        'replay_history',
        'rollback',
      ].includes(request.action)
    )
      throw Error('A USB-connected execution cannot rewind external host transactions.');
    switch (request.action) {
      case 'export_snapshot':
        return s.exportSnapshot().snapshot;
      case 'usb_status':
        return s.usbStatus(request.node);
      case 'usb_reset':
        return s.usbReset(request.node);
      case 'usb_setup':
        return s.usbSetup(request.node, request.bytes);
      case 'usb_out':
        return s.usbOut(request.node, request.endpoint, request.bytes);
      case 'usb_in':
        return s.usbIn(request.node, request.endpoint, request.size);
      case 'usb_in_token':
        return s.usbInToken(request.node, request.endpoint);
      case 'usb_step':
        // One explicit host scheduling quantum; status itself never advances time.
        await this.runUntil(
          s.status().ticks + BigInt(Math.max(1, Math.floor(this.manifest!.ticksPerSecond / 1000))),
        );
        this.rebase();
        return s.usbStatus(request.node);
      case 'press':
      case 'release': {
        if (this.manifest?.construction === 'resolved') {
          if (request.action === 'press' && request.stage)
            throw Error('Resolved key ABI does not support staged or touch input.');
          s.input(request.id, request.action === 'press', s.status().ticks);
        } else if (request.action === 'press') this.observe(s.press(request.id, request.stage));
        else this.observe(s.release(request.id));
        return;
      }
      case 'rotate':
        this.observe(s.rotate(request.id, request.steps));
        return;
      case 'touch':
        this.observe(s.touch(request.id, request.phase, request.x, request.y));
        return;
      case 'set_position':
        this.observe(s.setPosition(request.id, request.position));
        return;
      case 'scene':
        showScene(s, request.scene);
        this.scene = request.scene && 'rgb' in request.scene ? request.scene : undefined;
        return;
      case 'power':
        if (this.state?.poweredOn === request.on) return;
        await this.coldChange(() => this.observe(s.power(request.on)));
        if (this.config?.camera) await this.save();
        return;
      case 'power_cycle':
        await this.coldChange(() => this.observe(s.powerCycle(request.inputs)), request.inputs);
        return;
      case 'card_state':
        return s.cardState();
      case 'card_tree':
        return s.cardTree(request.revision);
      case 'card_read':
        return s.cardRead(request.path, request.options?.revision, request.options?.maxBytes);
      case 'card_insert':
        this.observe(s.cardInsert(request.present));
        return;
      case 'card_format':
        await this.coldChange(() => {
          this.observe(s.cardFormat());
          if (this.state?.poweredOn) this.observe(s.powerCycle(this.config?.bootInputs ?? []));
        }, this.config?.bootInputs);
        return;
      case 'restart_card': {
        const { rows, bytes } = packFiles(request.files);
        await this.coldChange(
          () => this.observe(s.replaceCard(rows, bytes, this.config?.bootInputs ?? [])),
          this.config?.bootInputs,
        );
        return;
      }
      case 'checkpoint':
        return this.captureCheckpoint();
      case 'restore_checkpoint':
        s.restore(request.id);
        this.afterRestore();
        return;
      case 'undo':
        s.undo();
        this.afterRestore();
        return this.state?.positions ?? {};
      case 'history':
        return s.history();
      case 'replay_history':
        s.replay(new Uint8Array(request.bytes));
        this.afterRestore();
        return;
      case 'fingerprint':
        return s.fingerprint();
      case 'rollback': {
        if (!Number.isFinite(request.seconds) || request.seconds <= 0)
          throw Error('Invalid rollback duration.');
        const now = s.status().ticks;
        const target = now - BigInt(Math.round(request.seconds * this.manifest!.ticksPerSecond));
        const retained =
          this.points.filter((point) => BigInt(point.tick) <= target).at(-1) ?? this.points[0];
        try {
          s.rollback(target < 0n ? 0n : target);
        } catch (error) {
          if (!(error instanceof Error) || !/retained history/.test(error.message) || !retained)
            throw error;
          s.restore(retained.id);
        }
        this.afterRestore();
        return this.state?.positions ?? {};
      }
      case 'profile':
        return {
          ...s.profile(request.limit),
          checkpoints: this.points,
          wasmBytes: this.module!.memoryBytes(),
          jit: this.module!.jitStatistics(),
          hostTimings: { worker: this.timings.snapshot(), load: this.loadTimings },
          observations: Object.fromEntries(
            (this.manifest?.observations ?? []).map((o) => [
              o.name,
              [...s.memoryRead(BigInt(o.address), o.size)],
            ]),
          ),
        };
      case 'device_states':
        return s.deviceStates(request.kind);
      case 'save':
        return this.save();
      case 'replay': {
        if (!this.replayMode) throw Error('Replay requires replay execution mode.');
        for (const step of request.steps) await this.replayStep(step);
        return;
      }
    }
  }
  private async errorReport() {
    if (!this.module) throw Error('Wasm session is not ready.');
    if (!this.camera)
      return {
        report: {
          runtime: 'wasm',
          runtimeKey: this.runtimeKey,
          firmwareId: this.config?.firmwareId,
          created: false,
          status: null,
          frame: null,
        },
      };
    const status = this.camera.status();
    const base = {
      runtime: 'wasm',
      runtimeKey: this.runtimeKey,
      ticks: String(status.ticks),
      instructions: String(status.instructions),
      failed: status.failed,
    };
    const { report, snapshot } = this.camera.errorReport();
    return {
      report: await diagnosticReport({ ...base, ...report }),
      snapshot: snapshot?.slice().buffer,
    };
  }
  private async loadSnapshot(bytes: Uint8Array, config: EngineConfig) {
    if (this.camera) throw Error('Wasm worker already booted.');
    this.config = config;
    const { module, manifest, identity, loadTimings, dmpWebGpu } = await this.prepare(config.bundle);
    this.loadTimings = loadTimings;
    this.module = module;
    this.dmpWebGpu = dmpWebGpu;
    const session = module.Camera.fromSnapshot(bytes);
    this.camera = session;
    this.manifest = manifest;
    this.runtimeKey = await digest(
      new TextEncoder().encode(`${identity}:${await digest(new Uint8Array(bytes))}`),
    );
    this.replayMode = config.execution?.mode !== 'live';
    this.failed = session.status().failed;
    this.observeCurrent();
    this.started = performance.now();
    this.startedTicks = session.status().ticks;
    this.speedSamples = [{ wall: this.started, ticks: this.startedTicks }];
    this.rebase();
    this.nextCheckpoint = this.startedTicks;
    this.publish(true);
    this.start();
    return (this.state?.inputs ?? manifest.inputs ?? []).map((input) => ({
      ...input,
      initial: this.state?.positions[input.id] ?? input.initial,
    }));
  }
  private async boot(
    dat: ArrayBuffer,
    config: EngineConfig,
    files: CardFile[],
    scene: Scene | undefined,
    seedIdentity: { path: string; sha256: string }[],
  ) {
    if (this.camera) throw Error('Wasm worker already booted.');
    this.config = config;
    const saved = config.camera ? await this.cameras.saved(config.camera.id) : undefined;
    this.scene = scene && 'rgb' in scene ? scene : undefined;
    this.send({ type: 'progress', message: 'Loading Wasm runtime…' });
    const { module, manifest, identity, loadTimings, dmpWebGpu } = await this.prepare(config.bundle);
    this.loadTimings = loadTimings;
    this.module = module;
    this.dmpWebGpu = dmpWebGpu;
    this.manifest = manifest;
    this.runtimeKey = await this.timings.measureAsync('runtimeIdentity', async () =>
      digest(
        new TextEncoder().encode(
          JSON.stringify([
            identity,
            manifest,
            config.firmwareId,
            await digest(dat),
            config.bootInputs ?? [],
            config.camera?.modEnabled ?? false,
            seedIdentity,
          ]),
        ),
      ),
    );
    this.send({
      type: 'progress',
      message: 'Building camera…',
      assets: [loadingStep('build', 'loading')],
    });
    this.replayMode = config.execution?.mode === 'replay';
    const now = new Date();
    const clockSeconds2000 = this.replayMode
      ? config.execution?.clockSeconds2000
      : Math.floor(
          (Date.UTC(
            now.getFullYear(),
            now.getMonth(),
            now.getDate(),
            now.getHours(),
            now.getMinutes(),
            now.getSeconds(),
          ) -
            Date.UTC(2000, 0, 1)) /
            1000,
        );
    const preferredLanguages = this.replayMode ? [] : [...navigator.languages];
    if (manifest.construction === 'prepared') {
      const prepared = manifest.prepared;
      if (!prepared || !config.bundle) throw Error('Prepared camera has no bundle files.');
      const { text, inputs } = await loadPreparedBundle(
        prepared,
        new URL('wasm.json', config.bundle),
      );
      this.camera = this.timings.measure('create', () =>
        createCamera(
          module,
          JSON.stringify({ document: text, clockSeconds2000, preferredLanguages }),
          new Uint8Array(dat),
          'prepared',
          inputs,
        ),
      );
    } else
      this.camera = this.timings.measure('create', () =>
        createCamera(
          module,
          manifest.construction === 'resolved'
            ? manifest.session
            : {
                schema: 1,
                graph: manifest.session,
                firmwareId: config.firmwareId,
                bootSource: manifest.session.bootSource,
                bootInputs: config.bootInputs ?? [],
                initialPositions: config.initialPositions ?? {},
                clockSeconds2000,
              },
          new Uint8Array(dat),
          manifest.construction,
        ),
      );
    const session = this.camera;
    if (manifest.construction !== 'resolved') {
      if (saved) this.observe(session.restoreDurable(new Uint8Array(saved.durable)));
      if (files.length) {
        const packed = packFiles(files);
        this.observe(session.replaceCard(packed.rows, packed.bytes, config.bootInputs ?? []));
      } else if (saved && (config.restart || config.bootInputs?.length))
        this.observe(session.powerCycle(config.bootInputs ?? []));
      this.observeCurrent();
    } else if (files.length || config.camera || Object.keys(config.initialPositions ?? {}).length)
      throw Error('Resolved ABI has no storage or initial positions.');
    this.send({
      type: 'progress',
      message: 'Booting camera…',
      assets: [loadingStep('build', 'complete'), bootProgress(0, manifest.bootTicks || undefined)],
    });
    if (scene && 'raf' in scene) {
      showScene(session, scene);
      scene = undefined;
    }
    await this.coldBoot(config.bootInputs);
    this.send({
      type: 'progress',
      message: 'Booting camera…',
      assets: [loadingStep('boot', 'complete')],
    });
    const inputs = (this.state?.inputs ?? manifest.inputs ?? []).map((input) => ({
      ...input,
      initial: this.state?.positions[input.id] ?? input.initial,
    }));
    if (config.camera && !this.replayMode) await this.autoSave();
    return inputs;
  }
  private async coldChange(change: () => void, held: BootInput[] = []) {
    const wasLive = this.live;
    this.stop();
    try {
      change();
      await this.coldBoot(held);
    } catch (error) {
      if (wasLive) {
        this.rebase();
        this.start();
      }
      throw error;
    }
  }
  private async coldBoot(held: BootInput[] = []) {
    const s = this.require();
    this.points = [];
    this.failed = false;
    this.nextInspection = 0n;
    this.nextPresentation = 0n;
    this.nextCheckpoint = 0n;
    if (this.scene) showScene(s, this.scene);
    if (this.state?.poweredOn !== false) {
      let lastProgress = -Infinity;
      const start = Number(s.status().ticks);
      const reportProgress = (ticks: number) => {
        const now = performance.now();
        if (now - lastProgress < 100) return;
        lastProgress = now;
        this.send({
          type: 'progress',
          message: 'Booting camera…',
          assets: [bootProgress(ticks - start, this.manifest!.bootTicks || undefined)],
        });
      };
      if (this.manifest!.bootTicks)
        await this.runUntil(s.status().ticks + BigInt(this.manifest!.bootTicks), reportProgress);
      if (this.manifest!.construction === 'prepared') {
        const report = await this.bootToRec(2_000_000_000n, reportProgress);
        if (report.status !== 'rec')
          throw Error(report.failure ?? `Camera boot stopped: ${report.status}`);
      }
      if (this.scene) showScene(s, this.scene);
      if (
        this.manifest!.construction === 'prepared' ||
        (this.manifest!.construction === 'session' && this.manifest!.session.preview)
      )
        s.preview();
      if (held.some((input) => input.position === undefined)) s.releaseBootInputs(held);
    }
    this.observeCurrent();
    this.started = performance.now();
    this.startedTicks = s.status().ticks;
    this.speedSamples = [{ wall: this.started, ticks: this.startedTicks }];
    this.rebase();
    if (this.manifest!.construction !== 'resolved' && !this.manifest!.usb) this.captureCheckpoint();
    this.publish(true);
    this.start();
  }
  private rebase() {
    this.clockWall = performance.now();
    this.clockTicks = this.require().status().ticks;
  }
  private start() {
    if (!this.replayMode && !this.failed && !this.live && this.state?.poweredOn !== false) {
      this.live = true;
      this.schedule();
    }
  }
  private stop() {
    this.live = false;
    clearTimeout(this.timer);
  }
  private schedule() {
    if (!this.live) return;
    this.timer = setTimeout(
      () =>
        this.enqueue(async () => {
          if (!this.live) return;
          try {
            const s = this.require(),
              rate = this.manifest!.ticksPerSecond,
              now = s.status().ticks;
            // Never race to recover missed wall time: after a stall (background tab, suspended
            // app) or a slow stretch the camera continues in real time. Only a stall restarts the
            // speed window, so a slow device still reads a smoothed speed.
            const wall = performance.now();
            const behind = ((wall - this.clockWall) * rate) / 1000 - Number(now - this.clockTicks);
            if (behind > rate / 4) {
              this.rebase();
              if (wall - this.lastPace > 1000) this.speedSamples = [{ wall, ticks: now }];
            }
            this.lastPace = wall;
            const target =
              this.clockTicks +
              BigInt(Math.max(0, Math.floor(((performance.now() - this.clockWall) * rate) / 1000)));
            const maximum = now + BigInt(Math.max(1, Math.floor(rate / 10)));
            if (target > now && this.state?.poweredOn !== false) {
              // A whole presentation quantum avoids tiny history transactions at the pacing edge.
              await this.timings.measureAsync('run', () => this.runUntil(maximum));
              if (
                this.manifest!.construction !== 'resolved' &&
                !this.manifest!.usb &&
                s.status().ticks >= this.nextCheckpoint
              )
                this.captureCheckpoint();
              if (performance.now() >= this.nextSave) await this.autoSave();
              if (s.status().ticks >= this.nextPresentation) {
                this.observeCurrent();
                this.publish();
              }
            } else if (performance.now() >= this.nextSave) await this.autoSave();
          } catch (error) {
            this.failed = true;
            this.stop();
            this.send({
              type: 'error',
              error: error instanceof Error ? error.message : String(error),
              fault: faultFrom(error),
            });
          }
          this.schedule();
        }),
      Math.max(
        0,
        Math.min(
          16,
          (Number(this.require().status().ticks - this.clockTicks) * 1000) /
            this.manifest!.ticksPerSecond -
            (performance.now() - this.clockWall),
        ),
      ),
    );
  }
  private captureCheckpoint(): Checkpoint {
    const { retainedIds, ...saved } = this.require().checkpoint();
    this.points = this.points.filter((point) => retainedIds.includes(point.id));
    this.points.push(saved);
    this.points = this.points.slice(-8);
    this.nextCheckpoint = BigInt(saved.tick) + BigInt(this.manifest!.ticksPerSecond * 2);
    return saved;
  }
  private afterRestore() {
    const tick = this.require().status().ticks;
    this.points = this.points.filter((point) => BigInt(point.tick) <= tick);
    this.nextCheckpoint = tick + BigInt(this.manifest!.ticksPerSecond * 2);
    this.started = performance.now();
    this.startedTicks = tick;
    this.speedSamples = [{ wall: this.started, ticks: tick }];
    this.nextInspection = tick;
    this.nextPresentation = tick;
    this.failed = this.require().status().failed;
    this.observeCurrent();
    this.rebase();
    this.publish(true);
    this.start();
  }
  private inspectSource(tick: bigint) {
    if (this.manifest!.construction === 'resolved' || tick < this.nextInspection) return;
    this.sourceFrames =
      this.manifest!.construction === 'prepared' || this.manifest!.session.preview
        ? this.require().previewState().frames
        : undefined;
    this.sourceTicks = tick;
    this.nextInspection = tick + BigInt(Math.round(this.manifest!.ticksPerSecond / 10));
  }
  private async replayStep(step: ReplayStep) {
    if (step.action === 'advance') {
      if (!Number.isSafeInteger(step.ticks) || step.ticks <= 0)
        throw Error('Invalid replay tick count.');
      if (this.state?.poweredOn === false) return;
      // Yield between bounded slices so inputs, disposal and browser messages remain responsive.
      const s = this.require(),
        end = s.status().ticks + BigInt(step.ticks);
      const quantum = BigInt(Math.max(1, Math.floor(this.manifest!.ticksPerSecond / 10)));
      while (s.status().ticks < end) {
        const next = s.status().ticks + quantum;
        await this.runUntil(next < end ? next : end);
        if (
          this.manifest!.construction !== 'resolved' &&
          !this.manifest!.usb &&
          s.status().ticks >= this.nextCheckpoint
        )
          this.captureCheckpoint();
        if (s.status().ticks >= this.nextPresentation) {
          this.observeCurrent();
          this.publish();
          await this.leased;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
      }
      this.observeCurrent();
      this.publish();
      // A following replay step must not mutate a shared borrowed frame.
      await this.leased;
    } else await this.dispatch(step);
  }
  private publish(force = false) {
    const s = this.require(),
      status = s.status(),
      wall = performance.now(),
      rate = this.manifest!.ticksPerSecond;
    if (!force && status.ticks < this.nextPresentation) return;
    this.inspectSource(status.ticks);
    this.nextPresentation = status.ticks + BigInt(Math.round(rate / 10));
    const elapsed = (wall - this.started) / 1000;
    this.send({
      type: 'metrics',
      metrics: {
        guestTicks: Number(status.ticks),
        virtualSeconds: Number(status.ticks - this.startedTicks) / rate,
        wallSeconds: elapsed,
        speed:
          wall > this.speedSamples[0]!.wall
            ? Number(status.ticks - this.speedSamples[0]!.ticks) /
              rate /
              ((wall - this.speedSamples[0]!.wall) / 1000)
            : 0,
        averageSpeed: elapsed > 0 ? Number(status.ticks - this.startedTicks) / rate / elapsed : 0,
        runtime: {
          instructions: String(status.instructions),
          ticksPerSecond: rate,
          runtimeKey: this.runtimeKey,
          sourceFrames: this.sourceFrames,
          sourceTicks: String(this.sourceTicks),
          checkpoints: this.points.map((point) => point.tick),
        },
      },
    });
    this.speedSamples.push({ wall, ticks: status.ticks });
    while (this.speedSamples.length > 2 && wall - this.speedSamples[1]!.wall >= 2000)
      this.speedSamples.shift();
    const frame = this.timings.measure('frame', () => s.frame(this.manifest!.output));
    if (!frame) return;
    const { rgba, width, height } = frame,
      sequence = Number(frame.sequence);
    frame.free();
    if (typeof SharedArrayBuffer !== 'undefined' && rgba.buffer instanceof SharedArrayBuffer) {
      this.leased = new Promise<void>((resolve) => {
        this.release = resolve;
      });
      this.send({
        type: 'frame_view',
        buffer: rgba.buffer,
        offset: rgba.byteOffset,
        length: rgba.length,
        width,
        height,
        sequence,
        lease: ++this.lease,
      });
    } else {
      if (!this.canvas || this.canvas.width !== width || this.canvas.height !== height)
        this.canvas = new OffscreenCanvas(width, height);
      const context = this.canvas.getContext('2d');
      if (!context) throw Error('Worker canvas is unavailable.');
      // Same backing memory: no slice, RGB conversion, PNG or pixel message clone.
      const view = new Uint8ClampedArray(
        rgba.buffer as ArrayBuffer,
        rgba.byteOffset,
        rgba.byteLength,
      );
      const bitmap = this.timings.measure('bitmap', () => {
        context.putImageData(new ImageData(view, width, height), 0, 0);
        return this.canvas!.transferToImageBitmap();
      });
      this.send({ type: 'frame', bitmap, width, height, sequence }, [bitmap]);
    }
  }
  private async autoSave() {
    try {
      await this.save();
      this.nextSave = performance.now() + 30000;
    } catch {
      this.nextSave = performance.now() + 10000;
    }
  }
  private async save() {
    const camera = this.config?.camera;
    try {
      if (!camera) throw Error('This session does not belong to a saved camera.');
      if (this.failed) throw Error('The stopped camera cannot replace its last good save.');
      const result = this.require().save();
      const record = await this.cameras.save(camera.id, camera.revision, {
        durable: result.durable.slice().buffer,
        runtimeKey: this.runtimeKey,
        poweredOn: result.poweredOn,
        positions: result.positions,
        modEnabled: camera.modEnabled,
      });
      camera.revision = record.revision;
      this.nextSave = performance.now() + 30000;
      this.send({ type: 'saved', savedAt: record.savedAt, revision: record.revision });
    } catch (error) {
      this.send({ type: 'saved', error: String(error) });
      throw error;
    }
  }
}
