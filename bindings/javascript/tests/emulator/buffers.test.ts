import assert from 'node:assert/strict';
import { test } from 'node:test';
import { copyBuffer, copySpan, packBuffers } from '../../dist/emulator/buffers.js';

test('buffer helpers copy borrowed spans and preserve file offsets', () => {
  const source = new Uint8Array([9, 1, 2, 8]);
  const copy = copyBuffer(source.subarray(1, 3));
  assert.deepEqual([...copy], [1, 2]);
  assert.notEqual(copy.buffer, source.buffer);
  assert.deepEqual([...copySpan(source, { offset: 1, length: 2 })], [1, 2]);
  assert.throws(() => copySpan(source, { offset: 3, length: 2 }), /Invalid buffer span/);

  const packed = packBuffers([
    { path: '/a', bytes: source.subarray(1, 3) },
    { path: '/b', bytes: new Uint8Array([3]) },
  ]);
  assert.deepEqual(packed.rows, [
    { path: '/a', offset: 0, length: 2 },
    { path: '/b', offset: 2, length: 1 },
  ]);
  assert.deepEqual([...packed.bytes], [1, 2, 3]);
});
