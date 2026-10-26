// Emscripten library imports: browser module and table operations only.
declare const Module: Record<string, unknown>;
declare const wasmMemory: WebAssembly.Memory;
declare const wasmTable: WebAssembly.Table;
declare const HEAPU8: Uint8Array;
declare const _uc_embedder_tci_wasm_load: WebAssembly.ImportValue;
declare const _uc_embedder_tci_wasm_store: WebAssembly.ImportValue;
declare const _uc_embedder_tci_wasm_call: WebAssembly.ImportValue;
declare function addToLibrary(library: Record<string, unknown>): void;

addToLibrary({
  unicorn_jit_host_grow__sig: 'ii',
  unicorn_jit_host_grow: function (count: number): number {
    try {
      return wasmTable.grow(count);
    } catch (error) {
      Module['unicornJitHostError'] = String(error);
      return -1;
    }
  },
  unicorn_jit_host_install__sig: 'iiiii',
  // Unicorn's TCI exports that generated modules import as env.load/store/helper.
  unicorn_jit_host_install__deps: [
    'uc_embedder_tci_wasm_load',
    'uc_embedder_tci_wasm_store',
    'uc_embedder_tci_wasm_call',
  ],
  unicorn_jit_host_install: function (
    pointer: number,
    length: number,
    exports: number,
    count: number,
  ): number {
    try {
      const module = new WebAssembly.Module(HEAPU8.slice(pointer, pointer + length));
      const instance = new WebAssembly.Instance(module, {
        env: {
          memory: wasmMemory,
          table: wasmTable,
          load: _uc_embedder_tci_wasm_load,
          store: _uc_embedder_tci_wasm_store,
          helper: _uc_embedder_tci_wasm_call,
        },
      });
      const view = new DataView(wasmMemory.buffer),
        decoder = new TextDecoder();
      const functions: { slot: number; fn: Function }[] = [];
      for (let n = 0; n < count; n++) {
        const at = exports + n * 12,
          name = view.getUint32(at, true),
          bytes = view.getUint32(at + 4, true),
          slot = view.getUint32(at + 8, true);
        const value = instance.exports[decoder.decode(HEAPU8.slice(name, name + bytes))];
        if (typeof value !== 'function')
          throw Error('Rust JIT requested a missing function export');
        functions.push({ slot, fn: value });
      }
      for (const { slot, fn } of functions) wasmTable.set(slot, fn);
      return 0;
    } catch (error) {
      Module['unicornJitHostError'] = String(error);
      return -1;
    }
  },
  unicorn_jit_host_clear__sig: 'vi',
  unicorn_jit_host_clear: function (slot: number): void {
    wasmTable.set(slot, null);
  },
  unicorn_jit_host_error__sig: 'iii',
  unicorn_jit_host_error: function (pointer: number, capacity: number): number {
    const bytes = new TextEncoder().encode(
      String(Module['unicornJitHostError'] ?? 'JIT host operation failed'),
    );
    if (capacity >= bytes.length && pointer !== 0) HEAPU8.set(bytes, pointer);
    return bytes.length;
  },
  unicorn_jit_host_now__sig: 'd',
  unicorn_jit_host_now: function (): number {
    return performance.now();
  },
});
