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
  Point,
  TouchPhase,
} from 'fujimod/emulator';
export type { EngineFault } from './fault';
export interface CardReadOptions {
  revision?: number;
  maxBytes?: number;
}
/** A retained restore point. */
export type Checkpoint = Point & { id: string };
/** An external connection the engine reports for display. */
export interface Connection {
  id: string;
  label: string;
  connection: 'connected' | 'disconnected' | 'unknown';
  support: 'modeled' | 'partial' | 'unmodeled';
  limits: string;
}
/** The only emulator contract used by the web app. All scheduling is engine-owned. */
/** Frames are borrowed only during the synchronous listener call. */
export type Frame = {
  width: number;
  height: number;
  sequence: number;
} & ({ rgb: Uint8Array } | { rgba: Uint8ClampedArray } | { bitmap: ImageBitmap });
export type Scene = { rgb: Uint8Array; width: number; height: number } | { raf: Uint8Array };
export interface CardFile {
  path: string;
  bytes: Uint8Array;
}
export interface BootMedia {
  identities: Promise<{ path: string; sha256: string }[]>;
  load(): Promise<CardFile[]>;
}
export type ReplayStep =
  | { action: 'press' | 'release'; id: string; stage?: string }
  | { action: 'set_position'; id: string; position: string }
  | { action: 'rotate'; id: string; steps: number }
  | { action: 'touch'; id: string; phase: TouchPhase; x: number; y: number }
  | { action: 'advance'; ticks: number };
export interface EngineConfig {
  camera?: { id: string; revision: number; modEnabled: boolean };
  firmwareId: string;
  bundle: string;
  sceneInput: boolean;
  initialPositions?: Record<string, string>;
  restart?: boolean;
  /** Physical contacts held before the first instruction of a cold boot. */
  bootInputs?: BootInput[];
  /** Cold boot with a fixed RTC; replay mode advances only through replay(). */
  execution?: { mode: 'live' | 'replay'; clockSeconds2000?: number };
}
export interface Engine {
  exportSnapshot(): Promise<Uint8Array>;
  loadSnapshot(
    bytes: Uint8Array,
    config: EngineConfig,
    progress: LoadingProgress,
  ): Promise<ControlInput[]>;
  checkpoint(): Promise<Checkpoint>;
  restoreCheckpoint(id: string): Promise<void>;
  undo(): Promise<Record<string, string>>;
  history(): Promise<InputLog>;
  replayHistory(bytes: Uint8Array): Promise<void>;
  fingerprint(): Promise<Fingerprint>;
  prepare(config: EngineConfig): void;
  saveCamera(): Promise<void>;
  cameraSaves(
    listener: (event: { savedAt?: number; revision?: number; error?: string }) => void,
  ): () => void;
  replay(steps: readonly ReplayStep[]): Promise<void>;
  metrics(listener: (metrics: CameraPerformance) => void): () => void;
  profile(limit?: number): Promise<Record<string, unknown>>;
  errors(listener: (fault: EngineFault) => void): () => void;
  rollback(seconds: number): Promise<Record<string, string>>;
  errorReport(submission?: Promise<ReportSubmission>): Promise<Blob>;
  boot(
    dat: ArrayBuffer,
    config: EngineConfig,
    progress: LoadingProgress,
    media?: BootMedia,
  ): Promise<ControlInput[]>;
  /** Restart with these files merged into existing media; matching paths are replaced. */
  restartWithCardFiles(files: CardFile[], progress: LoadingProgress): Promise<void>;
  formatCard(progress: LoadingProgress): Promise<void>;
  insertCard(present: boolean): Promise<void>;
  powerCycle(inputs: BootInput[], progress: LoadingProgress): Promise<void>;
  cardState(): Promise<CardState>;
  cardTree(revision?: number): Promise<CardTree>;
  readCardFile(path: string, options?: CardReadOptions): Promise<Uint8Array>;
  /** Live presence and modeled support, independently reported by the emulator. */
  connections(listener: (rows: Connection[]) => void): () => void;
  /** Media changes, including busy transitions; refresh the tree after writes settle. */
  cardChanges(listener: (state: CardState) => void): () => void;
  frames(listener: (frame: Frame) => void): () => void;
  press(id: string, stage?: string): Promise<void>;
  release(id: string): Promise<void>;
  rotate(id: string, steps: number): Promise<void>;
  touch(id: string, phase: TouchPhase, x: number, y: number): Promise<void>;
  setPosition(id: string, position: string): Promise<void>;
  setScene(scene: Scene | null): Promise<void>;
  power(on: boolean): Promise<void>;
  powerChanges(listener: (on: boolean) => void): () => void;
  dispose(): void;
}

declare global {
  interface Window {
    cameraEngine?: Engine;
  }
}
