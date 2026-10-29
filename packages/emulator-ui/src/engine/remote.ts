import type { LoadingProgress } from './download-progress';
import type { ReportSubmission } from './report';
import type { CameraPerformance } from './performance';
import type { EngineFault } from './fault';
import type {
  BootInput,
  CardState,
  CardTree,
  ControlInput,
  Fingerprint,
  InputLog,
  TouchPhase,
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
} from './api';

type SaveEvent = { savedAt?: number; revision?: number; error?: string };
export type NativeDisplayState = {
  source: 'stock' | 'glgui';
  title: string | null;
  sequence: number;
};
type Reply = { type: 'reply'; serial: number; result: unknown };
type Failure = { type: 'error'; serial?: number; error: string; fault?: EngineFault };
type NativeState = { type: 'native-state'; display: NativeDisplayState };
type FrameListeners = Set<(frame: Frame) => void>;

class Listeners<T> {
  private values = new Set<(value: T) => void>();
  add(listener: (value: T) => void) {
    this.values.add(listener);
    return () => this.values.delete(listener);
  }
  emit(value: T) {
    for (const listener of this.values) listener(value);
  }
}

const FRAME_HEADER_BYTES = 20;
const FRAME_MAGIC = 'FJMF';

/** Engine API client for a native session hosted by a development machine. */
export class RemoteEngine implements Engine {
  private socket?: WebSocket;
  private serial = 0;
  private pending = new Map<
    number,
    { resolve(value: unknown): void; reject(error: Error): void }
  >();
  private framesListeners: FrameListeners = new Set();
  private faultListeners = new Listeners<EngineFault>();
  private metricListeners = new Listeners<CameraPerformance>();
  private connectionListeners = new Listeners<Connection[]>();
  private cardListeners = new Listeners<CardState>();
  private saveListeners = new Listeners<SaveEvent>();
  private powerListeners = new Listeners<boolean>();
  private nativeStateListeners = new Listeners<NativeDisplayState>();
  private progress?: LoadingProgress;
  private unavailable?: Error;
  private disposed = false;
  private disposePromise?: Promise<void>;
  private frameSequence = 0;
  private latestFrame?: ArrayBuffer;
  private processingFrames = false;

  constructor(
    private readonly url: string,
    private readonly onError: (error: Error) => void,
  ) {}

  private async connect(): Promise<WebSocket> {
    if (this.disposed) throw Error('Emulator closed.');
    if (this.unavailable) throw this.unavailable;
    if (this.socket?.readyState === WebSocket.OPEN) return this.socket;
    if (this.socket?.readyState === WebSocket.CONNECTING) {
      const socket = this.socket;
      await new Promise<void>((resolve, reject) => {
        socket.addEventListener('open', () => resolve(), { once: true });
        socket.addEventListener(
          'error',
          () => reject(Error('Native emulator connection failed.')),
          {
            once: true,
          },
        );
      });
      return socket;
    }
    const socket = new WebSocket(this.url);
    socket.binaryType = 'arraybuffer';
    this.socket = socket;
    socket.addEventListener('message', (event: MessageEvent<ArrayBuffer | string>) => {
      if (typeof event.data === 'string') this.receiveJson(event.data);
      else this.queueFrame(event.data);
    });
    socket.addEventListener('error', () => this.fail(Error('Native emulator connection failed.')));
    socket.addEventListener('close', () => this.fail(Error('Native emulator connection closed.')));
    await new Promise<void>((resolve, reject) => {
      socket.addEventListener('open', () => resolve(), { once: true });
      socket.addEventListener(
        'error',
        () => reject(this.unavailable ?? Error('Native emulator connection failed.')),
        { once: true },
      );
    });
    return socket;
  }

  private queueFrame(packet: ArrayBuffer) {
    this.latestFrame = packet;
    if (this.processingFrames) return;
    this.processingFrames = true;
    void this.drainFrames();
  }

  private async drainFrames() {
    try {
      while (this.latestFrame) {
        const packet = this.latestFrame;
        this.latestFrame = undefined;
        await this.receiveFrame(packet);
      }
    } catch (error) {
      this.fail(error instanceof Error ? error : Error(String(error)));
    } finally {
      this.processingFrames = false;
      if (this.latestFrame) this.queueFrame(this.latestFrame);
    }
  }

  private fail(error: Error) {
    if (this.disposed || this.unavailable) return;
    this.unavailable = error;
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
    this.onError(error);
  }

  private receiveJson(source: string) {
    let message:
      | Reply
      | Failure
      | NativeState
      | { type: 'progress'; message: string; percent?: number };
    try {
      message = JSON.parse(source) as typeof message;
    } catch {
      this.fail(Error('Native emulator sent invalid JSON.'));
      return;
    }
    if (message.type === 'progress') {
      this.progress?.(message.message, message.percent);
      return;
    }
    if (message.type === 'native-state') {
      this.nativeStateListeners.emit(message.display);
      return;
    }
    if (message.type === 'error') {
      const error = message.fault ? Error(message.fault.message) : Error(message.error);
      if (message.serial !== undefined) {
        this.pending.get(message.serial)?.reject(error);
        this.pending.delete(message.serial);
      }
      if (message.fault) this.faultListeners.emit(message.fault);
      return;
    }
    const request = this.pending.get(message.serial);
    this.pending.delete(message.serial);
    request?.resolve(message.result);
  }

  private async receiveFrame(packet: ArrayBuffer) {
    if (this.disposed || packet.byteLength < FRAME_HEADER_BYTES) return;
    const bytes = new Uint8Array(packet);
    if (String.fromCharCode(...bytes.subarray(0, 4)) !== FRAME_MAGIC) {
      this.fail(Error('Native emulator sent an invalid LCD frame.'));
      return;
    }
    const header = new DataView(packet);
    const width = header.getUint32(4, false);
    const height = header.getUint32(8, false);
    const sequence = Number(header.getBigUint64(12, false));
    if (sequence <= this.frameSequence) return;
    const bitmap = await createImageBitmap(
      new Blob([bytes.subarray(FRAME_HEADER_BYTES)], { type: 'image/jpeg' }),
    );
    try {
      for (const listener of this.framesListeners) listener({ bitmap, width, height, sequence });
      this.frameSequence = sequence;
    } finally {
      bitmap.close();
    }
  }

  private async rpc<T>(action: string, args: Record<string, unknown> = {}): Promise<T> {
    const socket = await this.connect();
    const serial = ++this.serial;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(serial, {
        resolve: (value) => resolve(value as T),
        reject,
      });
      try {
        socket.send(JSON.stringify({ type: 'rpc', serial, action, args }));
      } catch (error) {
        this.pending.delete(serial);
        reject(error instanceof Error ? error : Error(String(error)));
      }
    });
  }

  private unavailableMethod<T>(name: string): Promise<T> {
    return Promise.reject(Error(`${name} is not available in the native live preview.`));
  }

  exportSnapshot(): Promise<Uint8Array> {
    return this.unavailableMethod('Snapshot export');
  }
  loadSnapshot(
    _bytes: Uint8Array,
    _config: EngineConfig,
    _progress: LoadingProgress,
  ): Promise<ControlInput[]> {
    return this.unavailableMethod('Snapshot restore');
  }
  checkpoint(): Promise<Checkpoint> {
    return this.unavailableMethod('Checkpoint');
  }
  restoreCheckpoint(_id: string): Promise<void> {
    return this.unavailableMethod('Checkpoint restore');
  }
  undo(): Promise<Record<string, string>> {
    return this.unavailableMethod('Undo');
  }
  history(): Promise<InputLog> {
    return this.unavailableMethod('Input history');
  }
  replayHistory(_bytes: Uint8Array): Promise<void> {
    return this.unavailableMethod('History replay');
  }
  fingerprint(): Promise<Fingerprint> {
    return this.unavailableMethod('Fingerprint');
  }
  prepare(_config: EngineConfig): void {}
  saveCamera(): Promise<void> {
    return this.unavailableMethod('Camera save');
  }
  cameraSaves(listener: (event: SaveEvent) => void) {
    return this.saveListeners.add(listener);
  }
  replay(_steps: readonly ReplayStep[]): Promise<void> {
    return this.unavailableMethod('Replay');
  }
  metrics(listener: (metrics: CameraPerformance) => void) {
    return this.metricListeners.add(listener);
  }
  profile(_limit?: number): Promise<Record<string, unknown>> {
    return this.unavailableMethod('Profile');
  }
  deviceStates(_kind: string): Promise<{ states: unknown[] }> {
    return this.unavailableMethod('Device states');
  }
  errors(listener: (fault: EngineFault) => void) {
    return this.faultListeners.add(listener);
  }
  rollback(_seconds: number): Promise<Record<string, string>> {
    return this.unavailableMethod('Rollback');
  }
  errorReport(_submission?: Promise<ReportSubmission>): Promise<Blob> {
    return this.unavailableMethod('Error report');
  }
  async boot(
    _dat: ArrayBuffer,
    config: EngineConfig,
    progress: LoadingProgress,
    _media?: BootMedia,
  ): Promise<ControlInput[]> {
    this.progress = progress;
    progress('Connecting to native emulator…');
    try {
      return await this.rpc<ControlInput[]>('boot', { config });
    } finally {
      this.progress = undefined;
    }
  }
  restartWithCardFiles(_files: CardFile[], _progress: LoadingProgress): Promise<void> {
    return this.unavailableMethod('Card restart');
  }
  formatCard(_progress: LoadingProgress): Promise<void> {
    return this.unavailableMethod('Card format');
  }
  insertCard(_present: boolean): Promise<void> {
    return this.unavailableMethod('Card insertion');
  }
  powerCycle(_inputs: BootInput[], _progress: LoadingProgress): Promise<void> {
    return this.unavailableMethod('Power cycle');
  }
  cardState(): Promise<CardState> {
    return this.unavailableMethod('Card state');
  }
  cardTree(_revision?: number): Promise<CardTree> {
    return this.unavailableMethod('Card tree');
  }
  readCardFile(_path: string, _options?: CardReadOptions): Promise<Uint8Array> {
    return this.unavailableMethod('Card file read');
  }
  connections(listener: (rows: Connection[]) => void) {
    return this.connectionListeners.add(listener);
  }
  cardChanges(listener: (state: CardState) => void) {
    return this.cardListeners.add(listener);
  }
  frames(listener: (frame: Frame) => void) {
    this.framesListeners.add(listener);
    return () => this.framesListeners.delete(listener);
  }
  press(id: string, stage?: string): Promise<void> {
    return this.rpc('press', stage === undefined ? { id } : { id, stage });
  }
  release(id: string): Promise<void> {
    return this.rpc('release', { id });
  }
  rotate(id: string, steps: number): Promise<void> {
    return this.rpc('rotate', { id, steps });
  }
  touch(id: string, phase: TouchPhase, x: number, y: number): Promise<void> {
    return this.rpc('touch', { id, phase, x, y });
  }
  setPosition(id: string, position: string): Promise<void> {
    return this.rpc('set_position', { id, position });
  }
  setScene(_scene: Scene | null): Promise<void> {
    return this.unavailableMethod('Scene input');
  }
  power(_on: boolean): Promise<void> {
    return this.unavailableMethod('Power input');
  }
  powerChanges(listener: (on: boolean) => void) {
    return this.powerListeners.add(listener);
  }
  nativeState(listener: (state: NativeDisplayState) => void) {
    return this.nativeStateListeners.add(listener);
  }
  dispose(): Promise<void> {
    if (this.disposePromise) return this.disposePromise;
    if (this.disposed) return Promise.resolve();
    this.disposed = true;
    const socket = this.socket;
    if (socket?.readyState !== WebSocket.OPEN) {
      socket?.close(1000, 'Preview closed');
      return Promise.resolve();
    }
    const serial = ++this.serial;
    this.disposePromise = new Promise<void>((resolve) => {
      const timeout = window.setTimeout(() => socket.close(1000, 'Preview closed'), 3000);
      const finish = () => {
        window.clearTimeout(timeout);
        resolve();
      };
      socket.addEventListener('close', finish, { once: true });
      socket.addEventListener('error', finish, { once: true });
      socket.send(JSON.stringify({ type: 'rpc', serial, action: 'close', args: {} }));
    });
    const error = Error('Emulator closed.');
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
    return this.disposePromise;
  }
}
