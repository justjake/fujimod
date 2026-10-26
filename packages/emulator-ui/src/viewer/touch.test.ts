import { test } from 'vitest';
import assert from 'node:assert/strict';
import { touchPoint } from './touch';
const pixels = { width: 720, height: 480 };
test('touch coordinates follow the contained image through scaling and letterboxing', () => {
  const bounds = { left: 10, top: 20, width: 900, height: 480 };
  assert.deepEqual(touchPoint(bounds, pixels, pixels, 110, 120), { inside: true, x: 10, y: 100 });
  assert.deepEqual(touchPoint(bounds, pixels, pixels, 20, 20), { inside: false, x: 0, y: 0 });
  assert.deepEqual(touchPoint(bounds, pixels, pixels, 910, 600), { inside: false, x: 719, y: 479 });
  assert.deepEqual(
    touchPoint({ left: 0, top: 0, width: 360, height: 300 }, pixels, pixels, 150, 130),
    { inside: true, x: 300, y: 200 },
  );
});
