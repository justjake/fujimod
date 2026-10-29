import type { LoadingProgress } from '@fujimod/emulator-ui/engine/download-progress';
import {
  HostTimings,
  type BootInput,
  type CardState,
  type CardTree,
  type ControlInput,
  type Fingerprint,
  type InputLog,
  type TouchPhase,
} from 'fujimod/emulator';
import type {
  BootMedia,
  CardFile,
  CardReadOptions,
  Checkpoint,
  Connection,
  Engine,
  EngineConfig,
  Frame,
  ReplayStep,
  Scene,
} from '@fujimod/emulator-ui/engine/api';
import { loadImageScene, loadRafScene } from './image-scene';
import { EngineError, faultFrom, type EngineFault } from '@fujimod/emulator-ui/engine/fault';
import type { CameraPerformance } from '@fujimod/emulator-ui/engine/performance';
import type { ReportSubmission } from '@fujimod/emulator-ui/engine/report';
import { zipBlob } from '@fujimod/emulator-ui/archive';
import type {
  WasmRequest,
  WasmWorkerEvent,
  WasmWorkerRequest,
} from '@fujimod/emulator-ui/engine/wasm-worker-protocol';

type SaveEvent = { savedAt?: number; revision?: number; error?: string };
class Listeners<T> {
  private listeners = new Set<(value: T) => void>();
  add(listener: (value: T) => void) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
  emit(value: T) {
    for (const listener of this.listeners) listener(value);
  }
}
/** Main-thread transport only; guest execution and pacing belong to the worker. */
export class WasmEngine implements Engine {
  async exportSnapshot(): Promise<Uint8Array> {
    return this.rpc<Uint8Array>({ action: 'export_snapshot' });
  }
  async loadSnapshot(bytes: Uint8Array, config: EngineConfig, progress: LoadingProgress) {
    if (this.booted) throw Error('Engine already booted.');
    this.progress = progress;
    try {
      const inputs = await this.rpc<ControlInput[]>({
        action: 'load_snapshot',
        bytes,
        config: this.absolute(config),
      });
      this.booted = true;
      this.sceneInput = config.sceneInput;
      this.firmwareId = config.firmwareId;
      return inputs;
    } finally {
      this.progress = undefined;
    }
  }
  private timings = new HostTimings();
  private worker: Worker;
  private serial = 0;
  private pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  private frameListeners = new Listeners<Frame>();
  private metricListeners = new Listeners<CameraPerformance>();
  private faultListeners = new Listeners<EngineFault>();
  private cardListeners = new Listeners<CardState>();
  private connectionListeners = new Listeners<Connection[]>();
  private saveListeners = new Listeners<SaveEvent>();
  private progress?: LoadingProgress;
  private fault?: EngineFault;
  private disposed = false;
  private unavailable?: Error;
  private selectedScene: Scene | null = null;
  private defaultScene?: Promise<Scene>;
  private sceneInput = false;
  private firmwareId?: string;
  private loadDefaultScene(): Promise<Scene> {
    if (this.firmwareId === 'x70-110') return loadRafScene();
    return (this.defaultScene ??= loadImageScene(
      new URL('./assets/default-scene.png', import.meta.url),
    ));
  }
  private booted = false;
  private presentedFrames = 0;
  private powerListeners = new Listeners<boolean>();
  private latestPower?: boolean;
  private latestCard?: CardState;
  private latestConnections?: Connection[];
  constructor(
    private onError: (error: Error) => void,
    createWorker = () =>
      new Worker(new URL('./wasm-worker.ts', import.meta.url), { type: 'module' }),
  ) {
    this.worker = createWorker();
    this.worker.onmessage = (event: MessageEvent<WasmWorkerEvent>) =>
      this.timings.measure(event.data.type, () => this.receive(event.data));
    this.worker.onerror = (event) => this.transportError(event.message || 'Wasm worker failed.');
    this.worker.onmessageerror = () =>
      this.transportError('Wasm worker message could not be decoded.');
  }
  /** Opaque host USB traffic; firmware execution remains inside the worker. */
  usb(request: Extract<WasmRequest, { node: string }>): Promise<unknown> {
    return this.rpc(request);
  }
  private transportError(message: string) {
    this.unavailable = Error(message);
    this.fail(faultFrom(this.unavailable));
  }
  private fail(fault: EngineFault) {
    if (this.fault || this.disposed) return;
    this.fault = fault;
    const error = new EngineError(fault);
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
    this.faultListeners.emit(fault);
    this.onError(error);
  }
  private receive(event: WasmWorkerEvent) {
    if (this.disposed) {
      if (event.type === 'frame') event.bitmap.close();
      return;
    }
    switch (event.type) {
      case 'reply': {
        const p = this.pending.get(event.serial);
        this.pending.delete(event.serial);
        p?.resolve(event.result);
        return;
      }
      case 'error': {
        if (event.serial !== undefined) {
          const p = this.pending.get(event.serial);
          this.pending.delete(event.serial);
          p?.reject(event.fault ? new EngineError(event.fault) : Error(event.error));
        }
        if (event.fault) this.fail(event.fault);
        return;
      }
      case 'progress':
        this.progress?.(event.message, event.percent, event.assets);
        return;
      case 'state':
        if (event.state.poweredOn !== this.latestPower) {
          this.latestPower = event.state.poweredOn;
          this.powerListeners.emit(this.latestPower);
        }
        this.latestCard = event.state.card;
        // The Wasm camera models no external host connections.
        this.latestConnections = [];
        this.cardListeners.emit(event.state.card);
        this.connectionListeners.emit([]);
        return;
      case 'metrics':
        this.metricListeners.emit({
          ...event.metrics,
          runtime: { ...event.metrics.runtime, presentedImageFrames: this.presentedFrames },
        });
        return;
      case 'saved':
        this.saveListeners.emit(event);
        return;
      case 'frame':
        try {
          this.frameListeners.emit({
            bitmap: event.bitmap,
            width: event.width,
            height: event.height,
            sequence: event.sequence,
          });
          this.presentedFrames++;
        } finally {
          event.bitmap.close();
        }
        return;
      case 'frame_view':
        try {
          this.frameListeners.emit({
            rgba: new Uint8ClampedArray(event.buffer, event.offset, event.length),
            width: event.width,
            height: event.height,
            sequence: event.sequence,
          });
          this.presentedFrames++;
        } finally {
          this.worker.postMessage({
            type: 'frame_release',
            lease: event.lease,
          } satisfies WasmWorkerRequest);
        }
    }
  }
  private rpc<T = void>(request: WasmRequest, transfer: Transferable[] = []): Promise<T> {
    if (this.disposed) return Promise.reject(Error('Emulator closed.'));
    if (this.unavailable) return Promise.reject(this.unavailable);
    const serial = ++this.serial;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(serial, { resolve: (value) => resolve(value as T), reject });
      try {
        this.worker.postMessage(
          { type: 'rpc', serial, request } satisfies WasmWorkerRequest,
          transfer,
        );
      } catch (error) {
        this.pending.delete(serial);
        reject(error);
      }
    });
  }
  private absolute(config: EngineConfig): EngineConfig {
    return {
      ...config,
      bundle: new URL(config.bundle, document.baseURI).href,
    };
  }
  checkpoint() {
    return this.rpc<Checkpoint>({ action: 'checkpoint' });
  }
  async restoreCheckpoint(id: string) {
    await this.rpc({ action: 'restore_checkpoint', id });
    this.fault = undefined;
  }
  async undo() {
    const positions = await this.rpc<Record<string, string>>({ action: 'undo' });
    this.fault = undefined;
    return positions;
  }
  history() {
    return this.rpc<InputLog>({ action: 'history' });
  }
  replayHistory(bytes: Uint8Array) {
    return this.rpc({ action: 'replay_history', bytes });
  }
  fingerprint() {
    return this.rpc<Fingerprint>({ action: 'fingerprint' });
  }
  prepare(config: EngineConfig) {
    void this.rpc({ action: 'prepare', bundle: this.absolute(config).bundle }).catch(() => {});
  }
  async boot(
    dat: ArrayBuffer,
    config: EngineConfig,
    progress: LoadingProgress,
    media?: BootMedia,
  ): Promise<ControlInput[]> {
    this.progress = progress;
    this.sceneInput = config.sceneInput;
    this.firmwareId = config.firmwareId;
    progress('Loading Wasm runtime…');
    const [files, seedIdentity] = await Promise.all([
      media?.load() ?? Promise.resolve([]),
      media?.identities ?? Promise.resolve([]),
    ]);
    const scene = this.sceneInput
      ? (this.selectedScene ?? (await this.loadDefaultScene()))
      : undefined;
    const inputs = await this.rpc<ControlInput[]>(
      { action: 'boot', dat, config: this.absolute(config), files, scene, seedIdentity },
      [dat],
    );
    if (this.selectedScene === scene && scene && 'raf' in scene) this.selectedScene = null;
    this.booted = true;
    return inputs;
  }
  frames(listener: (frame: Frame) => void) {
    return this.frameListeners.add(listener);
  }
  metrics(listener: (metrics: CameraPerformance) => void) {
    return this.metricListeners.add(listener);
  }
  errors(listener: (fault: EngineFault) => void) {
    if (this.fault) listener(this.fault);
    return this.faultListeners.add(listener);
  }
  connections(listener: (rows: Connection[]) => void) {
    if (this.latestConnections) listener(this.latestConnections);
    return this.connectionListeners.add(listener);
  }
  cardChanges(listener: (state: CardState) => void) {
    if (this.latestCard) listener(this.latestCard);
    return this.cardListeners.add(listener);
  }
  cameraSaves(listener: (event: SaveEvent) => void) {
    return this.saveListeners.add(listener);
  }
  saveCamera() {
    return this.rpc({ action: 'save' });
  }
  replay(steps: readonly ReplayStep[]) {
    return this.rpc({ action: 'replay', steps });
  }
  async profile(limit?: number) {
    const result = await this.rpc<Record<string, unknown>>({ action: 'profile', limit });
    return { ...result, mainTimings: this.timings.snapshot() };
  }
  deviceStates(kind: string) {
    return this.rpc<{ states: unknown[] }>({ action: 'device_states', kind });
  }
  async rollback(seconds: number) {
    const positions = await this.rpc<Record<string, string>>({ action: 'rollback', seconds });
    this.fault = undefined;
    return positions;
  }
  async errorReport(submission?: Promise<ReportSubmission>) {
    const exported = await this.rpc<{ report: Record<string, unknown>; snapshot?: ArrayBuffer }>({
      action: 'error_report',
    }).catch((error: unknown) => ({ report: { unavailable: String(error) }, snapshot: undefined }));
    const files: [string, Uint8Array<ArrayBuffer>][] = [
      [
        'report.json',
        new TextEncoder().encode(
          JSON.stringify(
            { ...exported.report, fault: this.fault, submission: await submission },
            null,
            2,
          ),
        ),
      ],
    ];
    if (exported.snapshot) files.push(['snapshot.emusnap', new Uint8Array(exported.snapshot)]);
    return zipBlob(files.map(([path, bytes]) => ({ path, data: new Blob([bytes]) })));
  }

  async restartWithCardFiles(files: CardFile[], progress: LoadingProgress) {
    this.progress = progress;
    progress('Restarting camera…');
    await this.rpc({ action: 'restart_card', files });
  }
  async formatCard(progress: LoadingProgress) {
    this.progress = progress;
    progress('Formatting card…');
    await this.rpc({ action: 'card_format' });
  }
  insertCard(present: boolean) {
    return this.rpc({ action: 'card_insert', present });
  }
  async powerCycle(inputs: BootInput[], progress: LoadingProgress) {
    this.progress = progress;
    progress('Restarting camera…');
    await this.rpc({ action: 'power_cycle', inputs });
  }
  cardState() {
    return this.rpc<CardState>({ action: 'card_state' });
  }
  cardTree(revision?: number) {
    return this.rpc<CardTree>({ action: 'card_tree', revision });
  }
  readCardFile(path: string, options?: CardReadOptions) {
    return this.rpc<Uint8Array>({ action: 'card_read', path, options });
  }
  press(id: string, stage?: string) {
    return this.rpc({ action: 'press', id, stage });
  }
  release(id: string) {
    return this.rpc({ action: 'release', id });
  }
  rotate(id: string, steps: number) {
    return this.rpc({ action: 'rotate', id, steps });
  }
  touch(id: string, phase: TouchPhase, x: number, y: number) {
    return this.rpc({ action: 'touch', id, phase, x, y });
  }
  setPosition(id: string, position: string) {
    return this.rpc({ action: 'set_position', id, position });
  }
  async setScene(scene: Scene | null) {
    this.selectedScene = scene;
    if (!this.booted) return;
    const image = scene ?? (await this.loadDefaultScene());
    if (this.selectedScene !== scene) return;
    await this.rpc({ action: 'scene', scene: image });
    if (this.selectedScene === scene && scene && 'raf' in scene) this.selectedScene = null;
  }
  powerChanges(listener: (on: boolean) => void) {
    if (this.latestPower !== undefined) listener(this.latestPower);
    return this.powerListeners.add(listener);
  }
  power(on: boolean) {
    return this.rpc({ action: 'power', on });
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.worker.terminate();
    for (const request of this.pending.values()) request.reject(Error('Emulator closed.'));
    this.pending.clear();
  }
}
