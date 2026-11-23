import { connectPeer, type PeerChannel } from '../../../../crates/usb-proxy/web/peer.ts';
import type { LoadingProgress } from './download-progress';
import type { ReportSubmission } from './report';
import type { CameraPerformance } from './performance';
import type { EngineFault } from './fault';
import type {
  BootInput,
  CardState,
  CardTree,
  ControlInput,
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
  source: 'stock' | 'focalized-ui';
  title: string | null;
  sequence: number;
};
export type NativeCatalog = {
  configuration: string;
  body: string;
  model: string;
  firmware: string;
  controls: unknown;
};
export type NativeSessionState = {
  inputs: ControlInput[];
  positions: Record<string, string>;
  poweredOn: boolean;
};
type Reply = { type: 'reply'; serial: number; result: unknown };
type Failure = { type: 'error'; serial?: number; error: string; fault?: EngineFault };
type NativeState = { type: 'native-state'; display: NativeDisplayState };
type NativeCatalogMessage = { type: 'catalog' } & Omit<NativeCatalog, 'controls'> & {
  controls: NativeCatalog['controls'];
};
type NativeSessionMessage = { type: 'state' } & NativeSessionState;
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

const text = new TextEncoder();
const decoder = new TextDecoder();

/**
 * Engine API client for a native session served in a share room: commands
 * and state on the room's data channel, the LCD as its video track.
 */
export class RemoteEngine implements Engine {
  private peer?: Promise<PeerChannel>;
  private video?: HTMLVideoElement;
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
  private catalogListeners = new Listeners<NativeCatalog>();
  private sessionListeners = new Listeners<NativeSessionState>();
  private latestCatalog?: NativeCatalog;
  private latestSession?: NativeSessionState;
  private progress?: LoadingProgress;
  private unavailable?: Error;
  private disposed = false;
  private disposePromise?: Promise<void>;
  private frameSequence = 0;

  constructor(
    private readonly url: string,
    private readonly onError: (error: Error) => void,
  ) {}

  /** Join a native session that publishes events before the first command. */
  async open(): Promise<void> {
    await this.connect();
  }

  private connect(): Promise<PeerChannel> {
    if (this.disposed) return Promise.reject(Error('Emulator closed.'));
    if (this.unavailable) return Promise.reject(this.unavailable);
    this.peer ??= connectPeer(this.url, { video: true }).then((peer) => {
      peer.onclose = () => this.fail(Error('Native emulator connection closed.'));
      void peer.send(text.encode(JSON.stringify({ type: 'hello' })));
      void this.receiveMessages(peer);
      void peer.video?.then((track) => this.showVideo(track));
      return peer;
    });
    this.peer.catch((error: unknown) =>
      this.fail(error instanceof Error ? error : Error(String(error))),
    );
    return this.peer;
  }

  private async receiveMessages(peer: PeerChannel) {
    while (!this.disposed) {
      let bytes: Uint8Array;
      try {
        bytes = await peer.receive();
      } catch {
        return;
      }
      this.receiveJson(decoder.decode(bytes));
    }
  }

  /** Publish each decoded video frame as an LCD image, at the display's own pace. */
  private showVideo(track: MediaStreamTrack) {
    const video = document.createElement('video');
    video.muted = true;
    video.playsInline = true;
    video.srcObject = new MediaStream([track]);
    this.video = video;
    const next = () => {
      if (this.disposed) return;
      video.requestVideoFrameCallback(async () => {
        if (video.videoWidth && video.videoHeight) {
          const bitmap = await createImageBitmap(video);
          try {
            const sequence = ++this.frameSequence;
            const frame = { bitmap, width: bitmap.width, height: bitmap.height, sequence };
            for (const listener of this.framesListeners) listener(frame);
          } finally {
            bitmap.close();
          }
        }
        next();
      });
    };
    next();
    void video.play().catch((error: unknown) =>
      this.fail(error instanceof Error ? error : Error(String(error))),
    );
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
      | NativeCatalogMessage
      | NativeSessionMessage
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
    if (message.type === 'catalog') {
      this.latestCatalog = message;
      this.catalogListeners.emit(message);
      return;
    }
    if (message.type === 'state') {
      this.latestSession = message;
      this.sessionListeners.emit(message);
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

  private async rpc<T>(action: string, args: Record<string, unknown> = {}): Promise<T> {
    const peer = await this.connect();
    const serial = ++this.serial;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(serial, {
        resolve: (value) => resolve(value as T),
        reject,
      });
      peer
        .send(text.encode(JSON.stringify({ type: 'rpc', serial, action, args })))
        .catch((error: unknown) => {
          this.pending.delete(serial);
          reject(error instanceof Error ? error : Error(String(error)));
        });
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
  completeState(): Promise<Uint8Array> {
    return this.unavailableMethod('Complete state');
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
  catalog(listener: (catalog: NativeCatalog) => void) {
    if (this.latestCatalog) listener(this.latestCatalog);
    return this.catalogListeners.add(listener);
  }
  sessionState(listener: (state: NativeSessionState) => void) {
    if (this.latestSession) listener(this.latestSession);
    return this.sessionListeners.add(listener);
  }
  dispose(): Promise<void> {
    if (this.disposePromise) return this.disposePromise;
    if (this.disposed) return Promise.resolve();
    this.disposed = true;
    const error = Error('Emulator closed.');
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
    this.video?.pause();
    if (this.video) this.video.srcObject = null;
    this.disposePromise = (this.peer ?? Promise.reject(error)).then(
      async (peer) => {
        const serial = ++this.serial;
        await peer
          .send(text.encode(JSON.stringify({ type: 'rpc', serial, action: 'close', args: {} })))
          .catch(() => {});
        peer.close();
      },
      () => {},
    );
    return this.disposePromise;
  }
}
