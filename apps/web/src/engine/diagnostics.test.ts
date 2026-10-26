import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { diagnosticReport } from './diagnostics';
import { digest } from './identity';
test('reports retain small registers and identify bulk data with its exact byte hash', async () => {
  const data = Uint8Array.from({ length: 8192 }, (_, i) => i % 256);
  const source = { address: 123, registers: [4, 5], pixels: [...data] };
  const result = (await diagnosticReport(source)) as {
    address: number;
    registers: number[];
    pixels: { sha256: string; length: number; preview: number[] };
  };
  assert.equal(result.address, 123);
  assert.deepEqual(result.registers, [4, 5]);
  assert.equal(result.pixels.sha256, await digest(data));
  assert.equal(result.pixels.length, data.length);
  assert.deepEqual(result.pixels.preview, [...data.slice(0, 32)]);
  assert.equal(source.pixels.length, data.length);
  assert(JSON.stringify(result).length < 1024);
});
