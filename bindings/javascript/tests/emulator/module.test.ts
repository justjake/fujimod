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
  const binary = new Uint8Array([0, 97, 115, 109]).buffer;
  await instantiateEmulatorModule(
    async (options) => {
      received = options;
      return { configure: (value: ModuleOptions) => configured.push(value), Camera: class {} };
    },
    binary,
    (path) => new URL(path, 'https://assets.example/runtime/camera.wasm').href,
    { executionPolicy: 'exact', tcgJit: false },
  );
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
    session: {},
    ticksPerSecond: 12_000_000,
    bootTicks: 0,
    output: 1,
    observations: [{ name: 'frame_count', address: '1234', size: 4 }],
  });
  assert.equal(parseEmulatorManifest({ ...manifest, bootInstructions: 42 }).bootInstructions, 42);
  assert.equal(manifest.bootInstructions, undefined);
  for (const bootInstructions of [0, -1, 1.5, NaN, Infinity, '42', Number.MAX_SAFE_INTEGER + 1])
    assert.throws(
      () => parseEmulatorManifest({ ...manifest, bootInstructions }),
      /Unsupported emulator manifest/,
    );
  assert.equal(manifest.output, 1);
  assert.equal(manifest.executionPolicy, 'exact');
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
