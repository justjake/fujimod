// The Emscripten library loads Rust-requested exports into the function table.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import { runInNewContext } from 'node:vm';
const source = (name: string) =>
  stripTypeScriptTypes(readFileSync(new URL(name, import.meta.url), 'utf8'), { mode: 'strip' });
// Browser TextDecoder rejects views over shared memory, unlike Node's decoder.
class BrowserTextDecoder extends TextDecoder {
  override decode(...args: Parameters<TextDecoder['decode']>) {
    const [input] = args;
    if (input) {
      const buffer = ArrayBuffer.isView(input) ? input.buffer : input;
      assert(!(buffer instanceof SharedArrayBuffer), 'TextDecoder input must be owned');
    }
    return super.decode(...args);
  }
}
const memory = new WebAssembly.Memory({ initial: 4, maximum: 32, shared: true }),
  table = new WebAssembly.Table({ initial: 2, element: 'anyfunc' });
let library: Record<string, unknown> = {};
const context = {
  Module: {} as Record<string, unknown>,
  _uc_embedder_tci_wasm_load: (...args: number[]) => {
    assert.deepEqual(args, [7, 8, 0, 9]);
    return 42n;
  },
  _uc_embedder_tci_wasm_store: () => assert.fail('the fixture imports only load'),
  _uc_embedder_tci_wasm_call: () => assert.fail('the fixture imports only load'),
  wasmMemory: memory,
  wasmTable: table,
  HEAPU8: new Uint8Array(memory.buffer),
  TextEncoder,
  TextDecoder: BrowserTextDecoder,
  DataView,
  WebAssembly,
  performance: { now: () => 123.5 },
  addToLibrary: (value: Record<string, unknown>) => {
    library = value;
  },
};
runInNewContext(source('./host.ts'), context);
const grow = library.unicorn_jit_host_grow as (count: number) => number;
const install = library.unicorn_jit_host_install as (
  pointer: number,
  length: number,
  exports: number,
  count: number,
) => number;
const clear = library.unicorn_jit_host_clear as (slot: number) => void;
const error = library.unicorn_jit_host_error as (pointer: number, length: number) => number;
assert.equal(grow(2), 2);
assert.equal(table.length, 4);
// A single handwritten Wasm fixture forwards its arguments to env.load.
const text = (value: string) => [value.length, ...new TextEncoder().encode(value)];
const section = (id: number, body: number[]) => [id, body.length, ...body];
const body = [0, 0x20, 0, 0x20, 1, 0x41, 0, 0x20, 2, 0x10, 0, 0x0b];
const fixture = new Uint8Array([
  0,
  97,
  115,
  109,
  1,
  0,
  0,
  0,
  ...section(1, [2, 0x60, 3, 0x7f, 0x7f, 0x7f, 1, 0x7e, 0x60, 4, 0x7f, 0x7f, 0x7f, 0x7f, 1, 0x7e]),
  ...section(2, [1, ...text('env'), ...text('load'), 0, 1]),
  ...section(3, [1, 0]),
  ...section(7, [1, ...text('fn'), 0, 1]),
  ...section(10, [1, body.length, ...body]),
]);
context.HEAPU8.set(fixture, 0x100);
context.HEAPU8.set(new TextEncoder().encode('fn'), 0x800);
const view = new DataView(memory.buffer);
[0x800, 2, 2].forEach((value, n) => view.setUint32(0x900 + n * 4, value, true));
assert.equal(install(0x100, fixture.length, 0x900, 1), 0);
assert.equal((table.get(2) as (a: number, b: number, c: number) => bigint)(7, 8, 9), 42n);
view.setUint32(0x904, 1, true);
assert.equal(install(0x100, fixture.length, 0x900, 1), -1);
assert.equal((table.get(2) as (a: number, b: number, c: number) => bigint)(7, 8, 9), 42n);
const length = error(0, 0);
assert(length > 0);
assert.equal(error(0xa00, length), length);
assert.match(
  new TextDecoder().decode(context.HEAPU8.subarray(0xa00, 0xa00 + length)),
  /missing function export/,
);
clear(2);
assert.equal(table.get(2), null);
assert.equal((library.unicorn_jit_host_now as () => number)(), 123.5);

console.log(
  JSON.stringify({
    passed: true,
    hostImports: true,
    rustExportManifest: true,
  }),
);
