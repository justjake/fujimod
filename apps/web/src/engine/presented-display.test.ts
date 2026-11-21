import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { PresentedDisplay } from './presented-display';

function frames(values: Record<number, bigint | undefined>) {
  return (output: number) => values[output];
}

test('shows the first output before any output presents', () => {
  const display = new PresentedDisplay([1, 0]);
  assert.deepEqual(display.observe(frames({})), { output: 1, sequence: 0 });
});

test('follows the output that presented most recently', () => {
  const display = new PresentedDisplay([1, 0]);
  assert.deepEqual(display.observe(frames({ 1: 1n })), { output: 1, sequence: 1 });
  assert.deepEqual(display.observe(frames({ 1: 1n, 0: 1n })), { output: 0, sequence: 2 });
  assert.deepEqual(display.observe(frames({ 1: 1n, 0: 1n })), { output: 0, sequence: 2 });
  assert.deepEqual(display.observe(frames({ 1: 2n, 0: 1n })), { output: 1, sequence: 3 });
});

test('keeps the current output when several present at once', () => {
  const display = new PresentedDisplay([1, 0]);
  display.observe(frames({ 0: 1n }));
  assert.deepEqual(display.observe(frames({ 1: 1n, 0: 2n })), { output: 0, sequence: 2 });
});

test('advances the sequence even when an output restarts its own count', () => {
  const display = new PresentedDisplay([1, 0]);
  display.observe(frames({ 1: 9n }));
  const switched = display.observe(frames({ 1: 9n, 0: 1n }));
  assert.equal(switched.output, 0);
  assert.equal(switched.sequence, 2);
});

test('needs at least one output', () => {
  assert.throws(() => new PresentedDisplay([]));
});
