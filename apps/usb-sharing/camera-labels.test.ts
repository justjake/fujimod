import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { labelName } from './camera-labels.ts';

await test('label names keep product spelling readable', () => {
  assert.deepEqual(
    ['X70', 'X-T30 II', 'GFX 50R', 'X half'].map(labelName),
    ['x70', 'x-t30-ii', 'gfx-50r', 'x-half'],
  );
});
await test('camera-labels.generated.json matches the catalog', () => {
  execFileSync(process.execPath, ['scripts/generate-camera-labels.ts', '--check'], {
    cwd: import.meta.dirname,
  });
});
