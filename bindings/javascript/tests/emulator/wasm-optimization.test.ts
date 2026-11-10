import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { optimizeWasm } from '../../emulator/wasm-optimization.mts';

await test('final optimization preserves shared memory, imported calls and atomic operations', () => {
  const sdk = process.env.EMSDK;
  assert.ok(sdk, 'Run with the pinned Emscripten SDK.');
  const directory = mkdtempSync(join(tmpdir(), 'fujimod-wasm-opt-'));
  try {
    const text = join(directory, 'input.wat');
    const binary = join(directory, 'input.wasm');
    writeFileSync(
      text,
      `(module
      (import "env" "operation" (func $operation (param i32) (result i32)))
      (memory (import "env" "memory") 1 2 shared)
      (func (export "answer") (param i32) (result i32)
        (i32.add (call $operation (local.get 0)) (i32.const 0)))
      (func (export "increment") (result i32)
        (i32.atomic.rmw.add (i32.const 0) (i32.const 1))))`,
    );
    const tool = join(
      sdk,
      'upstream',
      'bin',
      process.platform === 'win32' ? 'wasm-opt.exe' : 'wasm-opt',
    );
    const compiled = spawnSync(tool, [text, '--enable-threads', '-o', binary], {
      encoding: 'utf8',
    });
    assert.equal(compiled.status, 0, compiled.stderr);
    const execute = () => {
      const memory = new WebAssembly.Memory({ initial: 1, maximum: 2, shared: true });
      const module = new WebAssembly.Module(readFileSync(binary));
      const instance = new WebAssembly.Instance(module, {
        env: { memory, operation: (value: number) => value * 2 + 1 },
      });
      const answer = instance.exports.answer;
      const increment = instance.exports.increment;
      assert.equal(typeof answer, 'function');
      assert.equal(typeof increment, 'function');
      if (typeof answer !== 'function' || typeof increment !== 'function')
        throw Error('Missing exports');
      return [answer(10), increment(), increment(), new Int32Array(memory.buffer)[0]];
    };
    assert.deepEqual(execute(), [21, 0, 1, 2]);
    const receipt = optimizeWasm(binary, { emsdk: sdk, profiling: false, simd128: false });
    assert.deepEqual(execute(), [21, 0, 1, 2]);
    assert.equal(receipt.output_bytes, readFileSync(binary).length);
    assert.equal(receipt.output_sha256.length, 64);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
