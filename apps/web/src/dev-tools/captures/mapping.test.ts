import { expect, test } from 'vite-plus/test';
import { controlDiff } from './mapping';

const words = (...values: number[]) => new Uint8Array(Uint32Array.from(values).buffer);

test('candidates follow the control in both pairs; counters are only changed words', () => {
  const diff = controlDiff(0x1000, [
    words(0, 5, 1, 9),
    words(0, 6, 2, 9),
    words(0, 5, 3, 9),
    words(0, 6, 4, 9),
  ]);
  expect([...diff.candidates]).toEqual([0x1004]);
  expect([...diff.changed]).toEqual([0x1004, 5, 6, 5, 6, 0x1008, 1, 2, 3, 4]);
  expect(diff.omitted).toBe(0);
});

test('changed words beyond the limit are counted, not kept', () => {
  const diff = controlDiff(0, [words(1, 1), words(2, 2), words(1, 1), words(2, 2)], 1);
  expect(diff.changed.length).toBe(5);
  expect(diff.omitted).toBe(1);
  expect(diff.candidates.length).toBe(2);
});
