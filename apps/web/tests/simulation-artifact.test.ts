import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

for (const backend of ['webgl2', 'webgpu'])
  test(`optimized ${backend} artifact validates without experimental engine flags and stays within its size budget`, async () => {
    const bytes = await readFile(
      new URL(
        `../../../bindings/javascript/pkg/simulation/${backend}/fujimod_simulation_bg.wasm`,
        import.meta.url,
      ),
    );
    assert.ok(
      WebAssembly.validate(bytes),
      'Simulation Wasm requires unsupported or experimental instructions',
    );
    assert.ok(bytes.length <= 64 * 1024 * 1024, `Simulation Wasm exceeds 64 MiB: ${bytes.length}`);
  });
