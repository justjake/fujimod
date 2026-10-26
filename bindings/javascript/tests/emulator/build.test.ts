import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  encodeIntoSharedMemory,
  sharedStringEncoding,
} from '../../emulator/shared-memory-encoding.ts';
import {
  buildPlan,
  declarationModule,
  hoistBindgenDeclarations,
  options,
  pinnedToolchain,
  rustExports,
  shellWords,
} from '../../emulator/build.mts';

test('builder reads the pinned public toolchains from the package root configuration', () => {
  assert.deepEqual(pinnedToolchain(), { nightly: 'nightly-2026-10-10', emsdk: '6.0.13' });
});

test('generated UTF-8 encoding preserves shared-memory slices and code point boundaries', () => {
  const encoder = new class extends TextEncoder {
    override encodeInto(text: string, target: Uint8Array<ArrayBuffer>) {
      assert(target.buffer instanceof ArrayBuffer, 'Browser encoders refuse shared destinations');
      return super.encodeInto(text, target);
    }
  }();
  for (const [text, capacity] of [['MENU', 8], ['Aé🎞Z', 7], ['🎞', 3], ['é', 0]] as const) {
    const expected = new Uint8Array(capacity);
    const result = encoder.encodeInto(text, expected);
    const shared = new Uint8Array(new SharedArrayBuffer(capacity + 4));
    shared.fill(0xcc);
    assert.deepEqual(encodeIntoSharedMemory(encoder, text, shared.subarray(2, 2 + capacity)), result);
    assert.deepEqual(shared.subarray(2, 2 + result.written), expected.subarray(0, result.written));
    assert(shared.subarray(0, 2).every((byte) => byte === 0xcc));
    assert(shared.subarray(2 + result.written).every((byte) => byte === 0xcc));
  }
  const patched = sharedStringEncoding('const ret=cachedTextEncoder.encodeInto(arg,view);');
  assert.match(patched, /encodeIntoSharedMemory\(cachedTextEncoder,arg,view\)/);
  assert.doesNotMatch(patched, /cachedTextEncoder\.encodeInto/);
  assert.throws(() => sharedStringEncoding(''), /Expected one/);
});

test('build options keep artifacts in the requested output and validate tool flags', () => {
  const selected = options(
    ['--out', 'out/wasm', '--rust-simd128', '--c-simd128', '--allocator', 'emmalloc'],
    '/tmp/project',
  );
  assert.equal(selected.out, '/tmp/project/out/wasm');
  assert.equal(selected.rustSimd128, true);
  assert.equal(selected.cSimd128, true);
  assert.equal(selected.allocator, 'emmalloc');
  assert.throws(() => options(['--lto', 'invalid']), /Invalid --lto/);
  assert.throws(() => options(['--codegen-units', '0']), /positive integer/);
});

test('build plan targets the public static library and preserves encoded Rust flags', () => {
  const selected = options(['--out', '/tmp/emulator', '--rust-simd128'], '/tmp/project');
  const plan = buildPlan(selected, {
    CARGO_ENCODED_RUSTFLAGS: '-C\x1fdebuginfo=0',
    PATH: '/toolchain/bin',
    WASM_BINDGEN: '/tooling/bin/wasm-bindgen',
  });
  assert.equal(plan.env.PATH, '/tooling/bin:/toolchain/bin');
  assert.equal(plan.env.CARGO_TARGET_DIR, '/tmp/emulator/rust');
  assert.equal(plan.env.CAMERA_UNICORN_STATIC_DIR, '/tmp/emulator/unicorn');
  assert.deepEqual(plan.rustFlags, [
    '-C',
    'debuginfo=0',
    '-C',
    'target-feature=+atomics,+bulk-memory,+mutable-globals',
    '-C',
    'panic=abort',
    '-C',
    'target-feature=+simd128',
  ]);
  assert(plan.cFlags.includes('-pthread'));
  assert(plan.cmakeConfigure.includes('--fresh'));
  assert(plan.link.includes('-Oz'));
  assert(plan.link.includes('-pthread'));
  assert(plan.link.includes('-sPTHREAD_POOL_SIZE=2'));
  assert(plan.rust.includes('-Zbuild-std=std,panic_abort'));
  assert(plan.rust.at(-1)?.endsWith('/bindings/javascript/emulator/Cargo.toml'));
  assert(
    plan.link.includes(
      '/tmp/emulator/rust/wasm32-unknown-emscripten/dist/libfujimod_emulator_javascript.a',
    ),
  );
  assert(plan.link.includes('-sEXPORTED_FUNCTIONS=@/tmp/emulator/exports.json'));
  assert(plan.link.includes('-sWASM_BINDGEN'));
  assert(plan.link.includes('-sENVIRONMENT=web,worker,node'));
});

test('Rust flag tokenizer honors quoted groups without evaluating shell syntax', () => {
  assert.deepEqual(shellWords(`-C 'link-arg=-Wl,-u,symbol name' --cfg "feature=jit"`), [
    '-C',
    'link-arg=-Wl,-u,symbol name',
    '--cfg',
    'feature=jit',
  ]);
  assert.throws(() => shellWords("-C 'unfinished"), /Unclosed flag quote/);
});

test('exports keep wasm-bindgen symbols from Rust objects and leave C internal', () => {
  const nm = [
    'lib.a:fujimod_emulator_javascript-1a.o: camera_press_59a8',
    'lib.a:fujimod_emulator_javascript-1a.o: _RNvCs1_4core3fmt',
    'lib.a:wasm_bindgen-2b.o: __wbindgen_malloc',
    'lib.a:compiler_builtins-3c.o: __udivti3',
    'lib.a:cputlb.c.o: tlb_flush',
    'lib.a:js_sys-4d.o: anon.123',
    'lib.a:wasm_bindgen-2b.o: __wbindgen_malloc',
  ].join('\n');
  assert.deepEqual(rustExports(nm), ['___wbindgen_malloc', '_camera_press_59a8']);
});

const emitted = `interface WasmModule {
}

declare class Camera {
    private constructor();
}

interface BindgenModule {
  /* tslint:disable */
  /**
  * Configure once.
  */
    configure(options: ModuleOptions): void;
  /**
  * A card file.
  */
    export interface CardFile {
        path: string;
    }

    export type TouchPhase = "press" | "tap";

    Camera: typeof Camera;
}

export { BindgenModule };
export type MainModule = WasmModule & BindgenModule;
export default function MainModuleFactory (options?: unknown): Promise<MainModule>;
`;

test('bindgen declarations move out of the Emscripten module interface', () => {
  const hoisted = hoistBindgenDeclarations(emitted);
  assert.equal(
    hoisted.slice(
      hoisted.indexOf('/**\n* A card file'),
      hoisted.indexOf('interface BindgenModule'),
    ),
    '/**\n* A card file.\n*/\nexport interface CardFile {\n    path: string;\n}\n\n' +
      'export type TouchPhase = "press" | "tap";\n\n',
  );
  assert.match(
    hoisted,
    /interface BindgenModule \{\n {2}\/\* tslint:disable \*\/\n {2}\/\*\*\n {2}\* Configure once\.\n {2}\*\/\n {4}configure\(options: ModuleOptions\): void;\n {4}Camera: typeof Camera;\n\}/,
  );
  const module = declarationModule(hoisted);
  assert.match(module, /^\/\/ Generated by emulator\/build\.mts/);
  assert.match(
    module,
    /export type MainModuleFactory = \(options\?: unknown\) => Promise<MainModule>;\n$/,
  );
  assert.doesNotMatch(module, /export default function/);
  assert.throws(() => hoistBindgenDeclarations('interface WasmModule {}'), /no BindgenModule/);
});
