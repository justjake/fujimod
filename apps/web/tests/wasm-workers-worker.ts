import {
  loadEmulatorModule,
  type Camera,
  type DeviceWorkers,
  type Fingerprint,
} from 'fujimod/emulator';

interface Status {
  ticks: string;
  instructions: string;
  failed: boolean;
}
interface Pixels {
  width: number;
  height: number;
  sequence: string;
  sha256: string;
}
type Entry =
  | { kind: 'phase-start' | 'phase-end'; name: string; stage: string; status: Status }
  | { kind: 'run'; tick: string }
  | { kind: 'frame'; value: Pixels | null }
  | {
      kind: 'boundary';
      name: string;
      status: Status;
      fingerprint: Fingerprint;
    }
  | ({ kind: 'command' } & Command)
  | ({ kind: 'observe'; name?: string; result?: unknown; bytesSha256?: string } & Observation)
  | { kind: 'scene'; width: number; height: number; file: string; sha256: string };
/** Plan operations, each one Camera method. */
type Command =
  | { action: 'checkpoint' | 'preview' }
  | { action: 'restore'; options: { id: string } }
  | { action: 'set_position'; options: { id: string; position: string } }
  | { action: 'press'; options: { id: string; stage?: string } }
  | { action: 'release'; options: { id: string } };
type Observation =
  | { action: 'error_report' }
  | { action: 'device_states'; options: { kind: string } }
  | { action: 'memory_read'; options: { address: string; size: number } };
function execute(camera: Camera, command: Command) {
  switch (command.action) {
    case 'checkpoint':
      return camera.checkpoint();
    case 'preview':
      return camera.preview();
    case 'restore':
      return camera.restore(command.options.id);
    case 'set_position':
      return camera.setPosition(command.options.id, command.options.position);
    case 'press':
      return camera.press(command.options.id, command.options.stage);
    case 'release':
      return camera.release(command.options.id);
  }
}
function observe(
  camera: Camera,
  observation: Observation,
): { result?: unknown; bytes?: Uint8Array } {
  switch (observation.action) {
    case 'error_report':
      return { result: camera.errorReport().report };
    case 'device_states':
      return { result: camera.deviceStates(observation.options.kind) };
    case 'memory_read': {
      const { address, size } = observation.options;
      return { bytes: camera.memoryRead(BigInt(address), size) };
    }
  }
}
interface Photo {
  path: string;
  bytes: number;
  sha256: string;
}
interface Expectations {
  boundaries: number;
  frames: number;
  photos: number;
  minimumSpeed: number;
}
interface Fixture {
  schema: 1;
  ticksPerSecond: number;
  expected: Expectations;
  module: string;
  wasm: string;
  dat: string;
  wasmSha256: string;
  config: unknown;
  plan: Entry[];
  photos: Photo[];
  identity: unknown;
}
export interface WorkerPhase {
  name: string;
  stage: string;
  guestSeconds: number;
  wallSeconds: number;
  speed: number;
  heapBytes: number;
}
export interface CompleteWorkerReceipt {
  complete: true;
  identityPassed: true;
  workerPreference: {
    initial: DeviceWorkers;
    selected: DeviceWorkers;
  };
  wasmSha256: string;
  sourceIdentity: unknown;
  phases: WorkerPhase[];
  boundaries: number;
  frames: number;
  photos: Photo[];
  expected: Expectations;
  speedTargetPassed: boolean;
  photographicQualification: 'separate';
  jit: unknown;
  error?: string;
}
export type WorkerReceipt =
  | CompleteWorkerReceipt
  | {
      complete: false;
      identityPassed: false;
      phases: WorkerPhase[];
      error: string;
      lastEntry: EntryMemory | null;
    };
interface EntryMemory {
  index: number;
  kind: Entry['kind'];
  heapBytes: number;
  action?: string;
}
export type WorkerMessage =
  | { type: 'frame'; width: number; height: number; rgba: ArrayBuffer }
  | { type: 'photo'; path: string; bytes: ArrayBuffer }
  | { type: 'phase'; phase: WorkerPhase }
  | { type: 'result'; result: WorkerReceipt };
interface OwnerPort {
  onmessage: ((event: MessageEvent<{ workers: 0 | 2; plan: string }>) => void) | null;
  postMessage(message: WorkerMessage, transfer?: Transferable[]): void;
}
const owner = self as unknown as OwnerPort;
function check(value: unknown, message: string): asserts value {
  if (!value) throw Error(message);
}
function same(actual: unknown, expected: unknown, label: string) {
  const stable = (value: unknown) =>
    JSON.stringify(value, (_key, item: unknown) =>
      item !== null && typeof item === 'object' && !Array.isArray(item)
        ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
        : item,
    );
  check(
    stable(actual) === stable(expected),
    `${label}: ${stable(actual)} differs from ${stable(expected)}`,
  );
}
async function digest(bytes: Uint8Array) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes.slice().buffer))]
    .map((n) => n.toString(16).padStart(2, '0'))
    .join('');
}
async function bytes(url: string) {
  const response = await fetch(url);
  check(response.ok, `Artifact unavailable: ${url}`);
  return new Uint8Array(await response.arrayBuffer());
}
owner.onmessage = async (event: MessageEvent<{ workers: 0 | 2; plan: string }>) => {
  let session: Camera | undefined;
  const phases: WorkerPhase[] = [];
  let lastEntry: EntryMemory | null = null;
  let trapped = false;
  try {
    check(event.data.workers === 0 || event.data.workers === 2, 'Worker preference must be 0 or 2');
    const plan = new URL(event.data.plan, location.href);
    check(plan.origin === location.origin, 'Replay plan must have the same origin');
    const response = await fetch(plan);
    check(response.ok, 'Replay plan unavailable');
    const fixture = (await response.json()) as Fixture;
    check(fixture.schema === 1, 'Unsupported replay fixture');
    check(
      Number.isSafeInteger(fixture.ticksPerSecond) && fixture.ticksPerSecond > 0,
      'Invalid guest clock',
    );
    for (const count of [
      fixture.expected.boundaries,
      fixture.expected.frames,
      fixture.expected.photos,
    ])
      check(Number.isSafeInteger(count) && count >= 0, 'Invalid replay expectations');
    check(
      fixture.expected.boundaries > 0 && fixture.expected.frames > 0,
      'Replay needs complete machine and LCD observations',
    );
    check(
      Number.isFinite(fixture.expected.minimumSpeed) && fixture.expected.minimumSpeed > 0,
      'Invalid speed target',
    );
    let actualWasmSha256 = '';
    const [dat, module] = await Promise.all([
      bytes(fixture.dat),
      loadEmulatorModule(
        fixture.module,
        fixture.wasm,
        { executionPolicy: 'interactive' },
        async (input, init) => {
          const response = await fetch(input, init);
          actualWasmSha256 = await digest(new Uint8Array(await response.clone().arrayBuffer()));
          return response;
        },
      ),
    ]);
    same(actualWasmSha256, fixture.wasmSha256, 'Wasm binary identity');
    const scenes = new Map<string, Uint8Array>();
    for (const entry of fixture.plan) {
      if (entry.kind === 'scene' && !scenes.has(entry.file)) {
        const rgb = await bytes(entry.file);
        same(await digest(rgb), entry.sha256, 'Scene identity');
        scenes.set(entry.file, rgb);
      }
    }
    session = module.Camera.fromGraph(JSON.stringify(fixture.config), dat);
    const initial = session.deviceWorkers();
    const selected = session.deviceWorkers(event.data.workers);
    same(
      selected,
      { count: event.data.workers, effective: event.data.workers },
      'Selected executor',
    );
    const status = (): Status => {
      const s = session!.status();
      return {
        ticks: s.ticks.toString(),
        instructions: s.instructions.toString(),
        failed: s.failed,
      };
    };
    let started = 0,
      prior!: Status,
      boundaries = 0,
      frames = 0;
    for (const [index, entry] of fixture.plan.entries()) {
      lastEntry = {
        index,
        kind: entry.kind,
        heapBytes: module.memoryBytes(),
        ...('action' in entry ? { action: entry.action } : {}),
      };
      if (['scene', 'boundary'].includes(entry.kind)) console.log(JSON.stringify(lastEntry));
      switch (entry.kind) {
        case 'phase-start':
          same(status(), entry.status, `${entry.name} start`);
          prior = status();
          started = performance.now();
          break;
        case 'phase-end': {
          const wallSeconds = (performance.now() - started) / 1000,
            after = status();
          same(after, entry.status, `${entry.name} end`);
          const guestSeconds =
            Number(BigInt(after.ticks) - BigInt(prior.ticks)) / fixture.ticksPerSecond;
          const phase = {
            name: entry.name,
            stage: entry.stage,
            guestSeconds,
            wallSeconds,
            speed: guestSeconds / wallSeconds,
            heapBytes: module.memoryBytes(),
          };
          phases.push(phase);
          owner.postMessage({ type: 'phase', phase } satisfies WorkerMessage);
          break;
        }
        case 'run':
          session.runUntil(BigInt(entry.tick));
          break;
        case 'command':
          execute(session, entry);
          break;
        case 'observe': {
          const observed = observe(session, entry);
          const label = entry.name || entry.action;
          if (Object.hasOwn(entry, 'result')) same(observed.result, entry.result, label);
          if (entry.bytesSha256)
            same(await digest(observed.bytes!), entry.bytesSha256, `${label} bytes`);
          break;
        }
        case 'scene':
          session.setScene(entry.width, entry.height, scenes.get(entry.file)!);
          break;
        case 'frame': {
          const f = session.frame(1);
          const image = f && {
            width: f.width,
            height: f.height,
            sequence: f.sequence.toString(),
            rgba: f.rgba.slice(),
          };
          f?.free();
          const actual = image
            ? {
                width: image.width,
                height: image.height,
                sequence: image.sequence,
                sha256: await digest(image.rgba),
              }
            : null;
          same(actual, entry.value, `LCD observation ${frames}`);
          frames++;
          if (image)
            owner.postMessage(
              {
                type: 'frame',
                width: image.width,
                height: image.height,
                rgba: image.rgba.buffer,
              } satisfies WorkerMessage,
              [image.rgba.buffer],
            );
          break;
        }
        case 'boundary':
          same(session.fingerprint(), entry.fingerprint, `${entry.name} full Session`);
          same(status(), entry.status, `${entry.name} status`);
          same(session.deviceWorkers(), selected, 'Stable executor preference');
          boundaries++;
          break;
      }
    }
    same(boundaries, fixture.expected.boundaries, 'Complete boundary count');
    same(frames, fixture.expected.frames, 'LCD observation count');
    same(fixture.photos.length, fixture.expected.photos, 'Card photo count');
    check(
      phases.some((p) => p.stage === 'warm'),
      'Replay needs a warmed phase',
    );
    const fingerprint = session.fingerprint();
    for (const photo of fixture.photos) {
      // An owned copy, transferable to the owner.
      const data = new Uint8Array(session.cardRead(photo.path, undefined, 32 << 20));
      same(data.length, photo.bytes, 'JPEG size');
      same(await digest(data), photo.sha256, 'JPEG identity');
      owner.postMessage(
        { type: 'photo', path: photo.path, bytes: data.buffer } satisfies WorkerMessage,
        [data.buffer],
      );
    }
    same(session.fingerprint(), fingerprint, 'Card reads are observational');
    owner.postMessage({
      type: 'result',
      result: {
        complete: true,
        identityPassed: true,
        workerPreference: { initial, selected },
        wasmSha256: fixture.wasmSha256,
        sourceIdentity: fixture.identity,
        phases,
        boundaries,
        frames,
        photos: fixture.photos,
        expected: fixture.expected,
        speedTargetPassed: phases
          .filter((p) => p.stage === 'warm')
          .every((p) => p.speed >= fixture.expected.minimumSpeed),
        photographicQualification: 'separate',
        jit: module.jitStatistics(),
      },
    } satisfies WorkerMessage);
  } catch (error) {
    trapped = error instanceof WebAssembly.RuntimeError;
    owner.postMessage({
      type: 'result',
      result: { complete: false, identityPassed: false, phases, error: String(error), lastEntry },
    } satisfies WorkerMessage);
  } finally {
    // A trapped Wasm call may leave an owner borrow live; terminate its worker
    // instead of reentering the failed instance during cleanup.
    if (!trapped) session?.close();
  }
};
