import type {
  BootInput,
  ControlInput,
  ExecutionPolicyName,
  MainModule,
  ModuleOptions,
} from './camera.generated.ts';

/** A loaded module. Its API and types are generated from the Rust bindings. */
export type EmulatorModule = MainModule;
export type Camera = EmulatorModule['Camera']['prototype'];
export type Frame = EmulatorModule['Frame']['prototype'];
export type Status = EmulatorModule['Status']['prototype'];

export interface ModuleFactoryOptions {
  wasmBinary: ArrayBuffer;
  locateFile(path: string): string;
}

export type EmulatorModuleFactory = (options: ModuleFactoryOptions) => Promise<unknown>;

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object';
}

/** Instantiate a generated module and select its execution policy and JIT tier. */
export async function instantiateEmulatorModule(
  factory: EmulatorModuleFactory,
  wasmBinary: ArrayBuffer,
  locateFile: (path: string) => string,
  options: ModuleOptions = {},
): Promise<EmulatorModule> {
  const loaded = await factory({ wasmBinary, locateFile });
  if (
    !record(loaded) ||
    typeof loaded.configure !== 'function' ||
    typeof loaded.Camera !== 'function'
  )
    throw new Error('Generated module has no camera bindings.');
  const module = loaded as unknown as EmulatorModule;
  await module.configure(options);
  return module;
}

/** Load an Emscripten module and its Wasm binary in browsers or workers. */
export async function loadEmulatorModule(
  moduleUrl: string | URL,
  wasmUrl: string | URL,
  options: ModuleOptions = {},
  fetcher: typeof fetch = fetch,
): Promise<EmulatorModule> {
  const base = globalThis.location?.href;
  const moduleHref =
    moduleUrl instanceof URL
      ? moduleUrl.href
      : base
        ? new URL(moduleUrl, base).href
        : new URL(moduleUrl).href;
  const wasmHref = new URL(wasmUrl, moduleHref).href;
  const [moduleResponse, wasmResponse] = await Promise.all([
    import(/* @vite-ignore */ moduleHref),
    fetcher(wasmHref),
  ]);
  if (!wasmResponse.ok) throw new Error(`Wasm binary request failed (${wasmResponse.status}).`);
  const loaded: unknown = moduleResponse;
  if (!record(loaded) || typeof loaded.default !== 'function')
    throw new Error('Generated module has no factory.');
  return instantiateEmulatorModule(
    loaded.default as EmulatorModuleFactory,
    await wasmResponse.arrayBuffer(),
    (path) => new URL(path, wasmHref).href,
    options,
  );
}

/** Byte inputs of a prepared run. The update container is the `dat` argument. */
export interface PreparedInputs {
  bootRom: Uint8Array;
  nand?: Uint8Array;
  settings?: Uint8Array;
  /** The camera's zstd-compressed NAND sections by section name, passed through unchanged. */
  camera?: Record<string, Uint8Array>;
}

/** Construct a camera the way its manifest declares. */
export function createCamera(
  module: EmulatorModule,
  config: unknown,
  dat: Uint8Array,
  construction: EmulatorManifest['construction'],
  inputs?: PreparedInputs,
): Camera {
  switch (construction) {
    case 'resolved':
      return module.Camera.fromResolved(JSON.stringify(config), dat);
    case 'session':
      return module.Camera.fromGraph(JSON.stringify(config), dat);
    case 'prepared':
      // The document is shipped as its exact text: re-serializing parsed JSON would
      // turn integral floats into integers, which the native identity check rejects.
      if (typeof config !== 'string') throw new Error('Prepared document must be JSON text.');
      if (!inputs) throw new Error('Prepared session requires a boot ROM.');
      return createPrepared(module, config, dat, inputs);
  }
}

/** Camera sections travel packed in one span, listed in the request in order. */
function createPrepared(
  module: EmulatorModule,
  request: string,
  update: Uint8Array,
  inputs: PreparedInputs,
): Camera {
  const sections = Object.entries(inputs.camera ?? {});
  const camera = new Uint8Array(sections.reduce((sum, [, bytes]) => sum + bytes.length, 0));
  sections.reduce((offset, [, bytes]) => {
    camera.set(bytes, offset);
    return offset + bytes.length;
  }, 0);
  const parsed: unknown = JSON.parse(request);
  if (!record(parsed)) throw new Error('Prepared request must be a JSON object.');
  const text = sections.length
    ? JSON.stringify({ ...parsed, camera: sections.map(([name, bytes]) => [name, bytes.length]) })
    : request;
  return module.Camera.fromPrepared(
    text,
    update,
    inputs.bootRom,
    inputs.nand,
    inputs.settings,
    sections.length ? camera : undefined,
  );
}

/** Whether a thrown error reports a failed guest rather than a refused host request. */
export function isGuestExecutionError(error: unknown): error is Error {
  return error instanceof Error && error.name === 'GuestExecutionError';
}

/** Validate the versioned manifest before fetching a module or binary. */
export function parseEmulatorManifest(value: unknown): EmulatorManifest {
  if (!record(value)) throw new Error('Invalid emulator manifest.');
  const manifest = value as unknown as EmulatorManifest;
  if (
    manifest.schema !== 1 ||
    manifest.abi !== 1 ||
    typeof manifest.module !== 'string' ||
    manifest.module.length === 0 ||
    typeof manifest.wasm !== 'string' ||
    manifest.wasm.length === 0 ||
    !['resolved', 'session', 'prepared'].includes(manifest.construction) ||
    (manifest.construction === 'prepared' &&
      (!record(manifest.prepared) ||
        typeof manifest.prepared.document !== 'string' ||
        typeof manifest.prepared.bootRom !== 'string')) ||
    (manifest.executionPolicy !== undefined &&
      !['interactive', 'exact'].includes(manifest.executionPolicy)) ||
    (manifest.dmpCpuRenderer !== undefined && typeof manifest.dmpCpuRenderer !== 'boolean') ||
    !record(manifest.session) ||
    !Number.isSafeInteger(manifest.ticksPerSecond) ||
    manifest.ticksPerSecond <= 0 ||
    !Number.isSafeInteger(manifest.bootTicks) ||
    manifest.bootTicks < 0 ||
    !Number.isSafeInteger(manifest.output) ||
    manifest.output < 0 ||
    manifest.output > 0xffffffff
  )
    throw new Error('Unsupported emulator manifest.');
  if (manifest.observations !== undefined) {
    if (
      !Array.isArray(manifest.observations) ||
      manifest.observations.length > 64 ||
      manifest.observations.some(
        (observation: unknown) =>
          !record(observation) ||
          typeof observation.name !== 'string' ||
          !/^[a-zA-Z][a-zA-Z0-9_]*$/.test(observation.name) ||
          typeof observation.address !== 'string' ||
          !/^[0-9]+$/.test(observation.address) ||
          !Number.isSafeInteger(observation.size) ||
          (observation.size as number) < 1 ||
          (observation.size as number) > 4096,
      )
    )
      throw new Error('Invalid observation declaration.');
  }
  if (manifest.inputs !== undefined && !Array.isArray(manifest.inputs))
    throw new Error('Invalid emulator inputs.');
  if (
    manifest.usb !== undefined &&
    (!record(manifest.usb) || manifest.usb.kind !== 'exr2' || typeof manifest.usb.node !== 'string')
  )
    throw new Error('Invalid emulator USB configuration.');
  return manifest;
}

export interface EmulatorManifest {
  schema: 1;
  abi: 1;
  module: string;
  wasm: string;
  construction: 'resolved' | 'session' | 'prepared';
  /** Native prepared run: the document is JSON text, the boot ROM is raw bytes. */
  prepared?: { document: string; bootRom: string };
  /** Override the default policy for native-parity qualification bundles. */
  executionPolicy?: ExecutionPolicyName;
  /** Select the CPU rasterizer for DMP command lists. */
  dmpCpuRenderer?: boolean;
  session: Record<string, unknown>;
  ticksPerSecond: number;
  bootTicks: number;
  output: number;
  inputs?: ControlInput[];
  /** Optional external endpoint and boot inputs configured by the caller. */
  usb?: {
    kind: 'exr2';
    node: string;
    bootInputs?: BootInput[];
    initialPositions?: Record<string, string>;
  };
  observations?: { name: string; address: string; size: number }[];
}
