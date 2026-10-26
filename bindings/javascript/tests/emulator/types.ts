import type {
  Camera,
  CardTree,
  EmulatorManifest,
  EmulatorModule,
  Frame,
  JitStatistics,
  ModuleOptions,
  SessionState,
  Status,
} from '../../dist/emulator/index.js';
import type { OpenCamera } from '../../dist/emulator/node.mjs';

declare const module: EmulatorModule;
declare const camera: Camera;
declare const opened: OpenCamera;

const manifest: EmulatorManifest = {
  schema: 1,
  abi: 1,
  module: 'camera.mjs',
  wasm: 'camera.wasm',
  construction: 'session',
  session: {},
  ticksPerSecond: 12_000_000,
  bootTicks: 0,
  output: 1,
};
const options: ModuleOptions = { executionPolicy: 'interactive', tcgJit: { threshold: 2 } };
const status: Status = camera.status();
const ticks: bigint = status.ticks;
const frame: Frame | undefined = camera.frame(1);
const pixels: Uint8Array | undefined = frame?.rgba;
const state: SessionState = camera.press('menu');
const positions: Record<string, string> = state.positions;
const tree: CardTree = camera.cardTree(3);
const memory: Uint8Array = camera.memoryRead(4096n, 4);
const stats: JitStatistics = module.jitStatistics();
const count: number = stats.enabled ? stats.installed : 0;
const reopened: Camera = opened.module.Camera.fromSnapshot(camera.exportSnapshot().snapshot);
// @ts-expect-error Touch phases are a closed set.
camera.touch('screen', 'hover', 0, 0);
// @ts-expect-error Guest ticks are bigint.
camera.runUntil(1);
// @ts-expect-error Binary input stays Uint8Array.
camera.replay(new ArrayBuffer(1));
// @ts-expect-error Manifest ABI versions are exact.
const invalid: EmulatorManifest = { ...manifest, abi: 2 };
void options;
void ticks;
void pixels;
void positions;
void tree;
void memory;
void count;
void reopened;
void invalid;
