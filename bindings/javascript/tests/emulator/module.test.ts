import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { ModuleOptions } from '../../dist/emulator/index.js';
import {
  instantiateEmulatorModule,
  isGuestExecutionError,
  parseEmulatorManifest,
} from '../../dist/emulator/module.js';

test('module factory receives independent binary and asset resolution, then configures', async () => {
  let received: { wasmBinary: ArrayBuffer; locateFile(path: string): string } | undefined;
  const configured: ModuleOptions[] = [];
  let releaseConfiguration!: () => void;
  const configuration = new Promise<void>((resolve) => {
    releaseConfiguration = resolve;
  });
  let configurationStarted = false,
    moduleLoaded = false;
  const binary = new Uint8Array([0, 97, 115, 109]).buffer;
  const pending = instantiateEmulatorModule(
    async (options) => {
      received = options;
      return {
        configure: async (value: ModuleOptions) => {
          configurationStarted = true;
          await configuration;
          configured.push(value);
        },
        Camera: class {},
      };
    },
    binary,
    (path) => new URL(path, 'https://assets.example/runtime/camera.wasm').href,
    { executionPolicy: 'exact', tcgJit: false },
  );
  void pending.then(() => {
    moduleLoaded = true;
  });
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(configurationStarted, true);
  assert.equal(moduleLoaded, false);
  releaseConfiguration();
  await pending;
  assert.equal(received?.wasmBinary, binary);
  assert.equal(received?.locateFile('camera.wasm'), 'https://assets.example/runtime/camera.wasm');
  assert.deepEqual(configured, [{ executionPolicy: 'exact', tcgJit: false }]);
});

test('module validation rejects modules without camera bindings', async () => {
  await assert.rejects(
    instantiateEmulatorModule(
      async () => ({}),
      new ArrayBuffer(0),
      () => '',
    ),
    /no camera bindings/,
  );
});

test('guest failures are distinguished by error name', () => {
  const guest = new Error('guest faulted');
  guest.name = 'GuestExecutionError';
  assert.equal(isGuestExecutionError(guest), true);
  assert.equal(isGuestExecutionError(new Error('refused')), false);
});

test('manifest parsing validates the versioned runtime and observation shape', () => {
  const manifest = parseEmulatorManifest({
    schema: 1,
    abi: 1,
    module: 'camera.mjs',
    wasm: 'camera.wasm',
    construction: 'session',
    executionPolicy: 'exact',
    dmpCpuRenderer: true,
    session: {},
    ticksPerSecond: 12_000_000,
    bootTicks: 0,
    output: 1,
    observations: [{ name: 'frame_count', address: '1234', size: 4 }],
  });
  assert.equal(manifest.output, 1);
  assert.equal(manifest.executionPolicy, 'exact');
  assert.equal(manifest.dmpCpuRenderer, true);
  assert.equal(parseEmulatorManifest({ ...manifest, dmpCpuRenderer: false }).dmpCpuRenderer, false);
  assert.throws(
    () => parseEmulatorManifest({ ...manifest, dmpCpuRenderer: 'true' }),
    /Unsupported emulator manifest/,
  );
  assert.equal(
    parseEmulatorManifest({ ...manifest, executionPolicy: undefined }).executionPolicy,
    undefined,
  );
  assert.throws(
    () => parseEmulatorManifest({ ...manifest, executionPolicy: 'invalid' }),
    /Unsupported emulator manifest/,
  );
  assert.throws(
    () =>
      parseEmulatorManifest({
        ...manifest,
        observations: [{ name: '0bad', address: 'nope', size: 0 }],
      }),
    /Invalid observation declaration/,
  );
  assert.throws(
    () => parseEmulatorManifest({ ...manifest, abi: 2 }),
    /Unsupported emulator manifest/,
  );
});
