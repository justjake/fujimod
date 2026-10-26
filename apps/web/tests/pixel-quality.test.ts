import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { pixelQuality } from './pixel-quality';

const area = { left: 0, top: 0, right: 2, bottom: 2 };
const rgba = (values: number[]) => new Uint8ClampedArray(values.flatMap((v) => [v, v, v, 255]));

test('identical constant pixels cannot qualify photo content', () => {
  const pixels = rgba([127, 127, 127, 127]);
  const result = pixelQuality(pixels, pixels.slice(), 2, area);
  assert.deepEqual(result.mae, [0, 0, 0]);
  assert.deepEqual(result.correlation, [0, 0, 0]);
  assert.deepEqual(result.expectedVariance, [0, 0, 0]);
});

test('a constant reference does not match a different constant or textured image', () => {
  const reference = rgba([127, 127, 127, 127]);
  for (const actual of [rgba([128, 128, 128, 128]), rgba([126, 128, 126, 128])])
    assert.deepEqual(pixelQuality(reference, actual, 2, area).correlation, [0, 0, 0]);
});

test('textured images retain correlation and absolute-error checks', () => {
  const reference = rgba([10, 20, 30, 40]);
  assert.deepEqual(pixelQuality(reference, reference, 2, area).correlation, [1, 1, 1]);
  const shifted = pixelQuality(reference, rgba([30, 40, 50, 60]), 2, area);
  assert.deepEqual(shifted.correlation, [1, 1, 1]);
  assert.deepEqual(shifted.mae, [20, 20, 20]);
});
