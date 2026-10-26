import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  accessSync,
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { delimiter, dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gzipSync } from 'node:zlib';
import { parseArgs } from 'node:util';
import { sharedStringEncoding } from './shared-memory-encoding.ts';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const defaultOutput = resolve(root, '../../target/emulator');
const unicornLicenses = [
  'COPYING',
  'COPYING.LGPL2',
  'COPYING_GLIB',
  'qemu/COPYING',
  'qemu/COPYING.LIB',
  'qemu/LICENSE',
];

export interface BuildOptions {
  out: string;
  stagePackage: boolean;
  profiling: boolean;
  rustSimd128: boolean;
  cSimd128: boolean;
  lto?: string;
  optLevel?: string;
  codegenUnits?: number;
  allocator: 'dlmalloc' | 'emmalloc';
}

export interface BuildPlan {
  env: NodeJS.ProcessEnv;
  rustFlags: string[];
  cFlags: string[];
  rust: string[];
  cmakeConfigure: string[];
  cmakeBuild: string[];
  link: string[];
}

export function shellWords(value: string): string[] {
  const words: string[] = [];
  let word = '';
  let started = false;
  let quote: string | undefined;
  for (let index = 0; index < value.length; index++) {
    const character = value[index]!;
    if (quote) {
      if (character === quote) {
        quote = undefined;
      } else if (character === '\\' && quote === '"') {
        const next = value[++index];
        if (next === undefined) throw new Error('Trailing flag escape.');
        word += next === '"' || next === '\\' ? next : `\\${next}`;
      } else {
        word += character;
      }
    } else if (character === '"' || character === "'") {
      quote = character;
      started = true;
    } else if (character === '\\') {
      const next = value[++index];
      if (next === undefined) throw new Error('Trailing flag escape.');
      word += next;
      started = true;
    } else if (' \t\r\n'.includes(character)) {
      if (started) {
        words.push(word);
        word = '';
        started = false;
      }
    } else {
      word += character;
      started = true;
    }
  }
  if (quote) throw new Error('Unclosed flag quote.');
  if (started) words.push(word);
  return words;
}

export function options(args: string[], base = root): BuildOptions {
  const parsed = parseArgs({
    args,
    options: {
      out: { type: 'string', default: defaultOutput },
      'stage-package': { type: 'boolean', default: false },
      profiling: { type: 'boolean', default: false },
      'rust-simd128': { type: 'boolean', default: false },
      'c-simd128': { type: 'boolean', default: false },
      lto: { type: 'string' },
      'opt-level': { type: 'string' },
      'codegen-units': { type: 'string' },
      allocator: { type: 'string', default: 'dlmalloc' },
    },
  });
  const values = parsed.values;
  if (values.lto !== undefined && !['thin', 'fat', 'off'].includes(values.lto))
    throw new Error('Invalid --lto value.');
  if (
    values['opt-level'] !== undefined &&
    !['0', '1', '2', '3', 's', 'z'].includes(values['opt-level'])
  )
    throw new Error('Invalid --opt-level value.');
  const codegenUnits =
    values['codegen-units'] === undefined ? undefined : Number(values['codegen-units']);
  if (codegenUnits !== undefined && (!Number.isSafeInteger(codegenUnits) || codegenUnits < 1))
    throw new Error('--codegen-units must be a positive integer.');
  if (values.allocator !== 'dlmalloc' && values.allocator !== 'emmalloc')
    throw new Error('Invalid --allocator value.');
  return {
    out: resolve(base, values.out),
    stagePackage: values['stage-package'],
    profiling: values.profiling,
    rustSimd128: values['rust-simd128'],
    cSimd128: values['c-simd128'],
    lto: values.lto,
    optLevel: values['opt-level'],
    codegenUnits,
    allocator: values.allocator,
  };
}

/** The wasm-bindgen CLI matching Cargo.lock; `npm run setup:tooling` installs it. */
export function wasmBindgen(source: NodeJS.ProcessEnv): string {
  return source.WASM_BINDGEN ?? resolve(root, '../../target/tooling/bin/wasm-bindgen');
}

/**
 * wasm-bindgen 0.2.129 nests tsify's `export interface` and `export type`
 * declarations inside the Emscripten `BindgenModule` interface, which is not
 * valid TypeScript. Move each one, with its comments, before that interface.
 */
export function hoistBindgenDeclarations(text: string): string {
  const open = text.indexOf('interface BindgenModule {\n');
  if (open < 0) throw new Error('Emitted declarations have no BindgenModule.');
  const bodyStart = open + 'interface BindgenModule {\n'.length;
  const bodyEnd = text.indexOf('\n}\n', bodyStart);
  if (bodyEnd < 0) throw new Error('Emitted BindgenModule is not terminated.');
  const lines = text.slice(bodyStart, bodyEnd).split('\n');
  const hoisted: string[] = [];
  const members: string[] = [];
  let pending: string[] = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index]!;
    const trimmed = line.trim();
    if (!trimmed) continue;
    if (/^(\/\*|\*|\/\/)/.test(trimmed)) {
      pending.push(trimmed);
      continue;
    }
    if (!/^export (interface|type) /.test(trimmed)) {
      members.push(...pending.map((comment) => `  ${comment}`), line);
      pending = [];
      continue;
    }
    const declaration = [...pending, trimmed];
    pending = [];
    let depth = 0;
    const count = (value: string) => {
      for (const character of value) {
        if (character === '{') depth++;
        else if (character === '}') depth--;
      }
    };
    count(trimmed);
    const typeAlias = trimmed.startsWith('export type ');
    while (depth > 0 || (typeAlias && !declaration.at(-1)!.endsWith(';'))) {
      const next = lines[++index];
      if (next === undefined) throw new Error('Unterminated bindgen declaration.');
      declaration.push(next.replace(/^ {4}/, ''));
      count(next);
    }
    hoisted.push(declaration.join('\n'));
  }
  return (
    text.slice(0, open) +
    hoisted.map((declaration) => `${declaration}\n\n`).join('') +
    'interface BindgenModule {\n' +
    members.join('\n') +
    text.slice(bodyEnd)
  );
}

/**
 * Every unmangled symbol a Rust object defines: the wasm-bindgen exports and
 * their descriptors. Unicorn's C objects and compiler builtins stay internal,
 * so the linker can drop what the emulator never calls.
 */
export function rustExports(nm: string): string[] {
  const exports = new Set<string>();
  for (const line of nm.split('\n')) {
    const match = /^[^:]+:([^:]+): (\S+)$/.exec(line.trim());
    if (!match) continue;
    const [, member, symbol] = match as unknown as [string, string, string];
    if (member.endsWith('.c.o') || member.startsWith('compiler_builtins-')) continue;
    if (/^(_Z|_R|anon\.)/.test(symbol)) continue;
    exports.add(`_${symbol}`);
  }
  return [...exports].sort();
}

const factory =
  'export default function MainModuleFactory (options?: unknown): Promise<MainModule>;\n';
/** The emitted declarations as a TypeScript module that `tsc` emits with the package. */
export function declarationModule(declarations: string): string {
  if (!declarations.endsWith(factory)) throw new Error('Emitted declarations end unexpectedly.');
  return (
    '// Generated by emulator/build.mts from the Rust bindings; do not edit.\n' +
    declarations.slice(0, -factory.length) +
    'export type MainModuleFactory = (options?: unknown) => Promise<MainModule>;\n'
  );
}

export function buildPlan(selected: BuildOptions, source: NodeJS.ProcessEnv): BuildPlan {
  const out = resolve(selected.out);
  const target = join(out, 'rust');
  const unicorn = join(out, 'unicorn');
  const env: NodeJS.ProcessEnv = {
    ...source,
    CARGO_TARGET_DIR: target,
    CAMERA_UNICORN_STATIC_DIR: unicorn,
    PATH: [dirname(wasmBindgen(source)), source.PATH].filter(Boolean).join(delimiter),
  };
  if (selected.lto !== undefined) env.CARGO_PROFILE_RELEASE_LTO = selected.lto;
  if (selected.optLevel !== undefined) env.CARGO_PROFILE_RELEASE_OPT_LEVEL = selected.optLevel;
  if (selected.codegenUnits !== undefined)
    env.CARGO_PROFILE_RELEASE_CODEGEN_UNITS = String(selected.codegenUnits);

  const rustFlags =
    env.CARGO_ENCODED_RUSTFLAGS !== undefined
      ? env.CARGO_ENCODED_RUSTFLAGS.split('\x1f')
      : shellWords(env.RUSTFLAGS ?? '');
  rustFlags.push('-C', 'target-feature=+atomics,+bulk-memory,+mutable-globals');
  // wasm-bindgen does not support unwinding on Emscripten; a panic traps the module.
  rustFlags.push('-C', 'panic=abort');
  if (selected.rustSimd128) rustFlags.push('-C', 'target-feature=+simd128');
  env.CARGO_ENCODED_RUSTFLAGS = rustFlags.join('\x1f');
  delete env.RUSTFLAGS;

  const simdFlags = selected.rustSimd128 || selected.cSimd128 ? ['-msimd128'] : [];
  const cFlags = [
    '-fwasm-exceptions',
    '-sSUPPORT_LONGJMP=wasm',
    '-DCONFIG_TCI_WASM_TIER',
    '-pthread',
    ...(selected.cSimd128 ? ['-msimd128'] : []),
  ];
  const rust = [
    'cargo',
    '-Zbuild-std=std,panic_abort',
    'build',
    '--locked',
    '-j2',
    '--profile',
    'dist',
    '--lib',
    '--target',
    'wasm32-unknown-emscripten',
    '--manifest-path',
    join(root, 'emulator/Cargo.toml'),
  ];
  const cmakeConfigure = [
    'emcmake',
    'cmake',
    '--fresh',
    '-S',
    join(root, '../../crates/emulator-cpu-unicorn/vendor/unicorn'),
    '-B',
    unicorn,
    '-G',
    'Ninja',
    '-DCMAKE_BUILD_TYPE=Release',
    '-DUNICORN_ARCH=arm',
    '-DUNICORN_TCG_INTERPRETER=ON',
    '-DUNICORN_BUILD_TESTS=OFF',
    '-DUNICORN_INSTALL=OFF',
    '-DUNICORN_LEGACY_STATIC_ARCHIVE=OFF',
    '-DBUILD_SHARED_LIBS=OFF',
    `-DCMAKE_C_FLAGS=${cFlags.join(' ')}`,
  ];
  const cmakeBuild = ['ninja', '-j2', '-C', unicorn, 'unicorn'];
  const link = [
    'emcc',
    join(target, 'wasm32-unknown-emscripten/dist/libfujimod_emulator_javascript.a'),
    '-o',
    join(out, 'camera.mjs'),
    // Keep the standalone module within static hosting's per-file size budget.
    '-Oz',
    '--no-entry',
    '-pthread',
    '-sPTHREAD_POOL_SIZE=2',
    '-sPTHREAD_POOL_SIZE_STRICT=2',
    '-sDEFAULT_PTHREAD_STACK_SIZE=4194304',
    '-sALLOW_BLOCKING_ON_MAIN_THREAD=0',
    ...simdFlags,
    '-fwasm-exceptions',
    '-sSUPPORT_LONGJMP=wasm',
    '-sMODULARIZE=1',
    '-sEXPORT_NAME=createEmulator',
    '-sENVIRONMENT=web,worker,node',
    '-sALLOW_MEMORY_GROWTH=1',
    '-sINITIAL_MEMORY=33554432',
    '-sMAXIMUM_MEMORY=2147483648',
    '-sSTACK_SIZE=2097152',
    '-sFILESYSTEM=0',
    '-sNO_EXIT_RUNTIME=1',
    '-sALLOW_TABLE_GROWTH=1',
    `-sMALLOC=${selected.allocator}`,
    '--js-library',
    join(out, 'jit/host.js'),
    // emcc runs wasm-bindgen on the linked module and emits its declarations.
    '-sWASM_BINDGEN',
    '-Wno-experimental',
    '--emit-tsd',
    'camera.d.ts',
    `-sEXPORTED_FUNCTIONS=@${join(out, 'exports.json')}`,
    ...(selected.profiling ? ['--profiling-funcs'] : []),
  ];
  return { env, rustFlags, cFlags, rust, cmakeConfigure, cmakeBuild, link };
}

export function pinnedToolchain(): { nightly: string; emsdk: string } {
  const config = readFileSync(resolve(root, '../../mise.toml'), 'utf8');
  const nightly = config.match(/version\s*=\s*"(nightly-[0-9-]+)"/)?.[1];
  const emsdk = config.match(/^emsdk\s*=\s*"([^"]+)"/m)?.[1];
  if (!nightly || !emsdk)
    throw new Error('Public Mise config must pin nightly Rust and Emscripten.');
  return { nightly, emsdk };
}

function requireTool(tool: string): void {
  const pathSeparator = sep === '\\' ? ';' : ':';
  const available = (process.env.PATH ?? '').split(pathSeparator).some((directory) => {
    try {
      accessSync(join(directory, tool), constants.X_OK);
      return true;
    } catch {
      return false;
    }
  });
  if (!available) throw new Error(`${tool} is required; install the pinned tools in mise.toml.`);
}

function capture(args: string[], cwd = root): string {
  const result = spawnSync(args[0]!, args.slice(1), {
    cwd,
    encoding: 'utf8',
    maxBuffer: 256 << 20,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr || `${args[0]} failed.`);
  return result.stdout;
}

function run(args: string[], env: NodeJS.ProcessEnv): void {
  console.log('+', args.join(' '));
  const result = spawnSync(args[0]!, args.slice(1), { cwd: root, env, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${args[0]} exited ${result.status ?? result.signal}.`);
}

export function validateToolchain(pinned = pinnedToolchain()): void {
  const emccVersion = capture(['emcc', '--version']).split('\n')[0] ?? '';
  if (!emccVersion.includes(` ${pinned.emsdk}`))
    throw new Error(`Expected Emscripten ${pinned.emsdk}; got ${emccVersion}.`);
  const installedToolchains = capture(['rustup', 'toolchain', 'list']);
  if (!installedToolchains.split('\n').some((line) => line.startsWith(`${pinned.nightly}-`)))
    throw new Error(`Expected installed Rust toolchain ${pinned.nightly}.`);
  const rustcVersion = capture(['rustup', 'run', pinned.nightly, 'rustc', '--version']);
  if (!rustcVersion.startsWith('rustc '))
    throw new Error(`Could not run Rust toolchain ${pinned.nightly}: ${rustcVersion.trim()}.`);
}

function validateSources(): void {
  const vendor = join(root, '../../crates/emulator-cpu-unicorn/vendor/unicorn');
  for (const path of [
    join(root, 'emulator/Cargo.toml'),
    join(root, 'jit/host.ts'),
    ...unicornLicenses.map((name) => join(vendor, name)),
  ]) {
    if (!existsSync(path)) throw new Error(`Required emulator build input is missing: ${path}`);
  }
}

function stageRuntimeNotices(out: string): void {
  copyFileSync(join(root, 'NOTICE'), join(out, 'NOTICE'));
  const licenseDirectory = join(out, 'licenses');
  const vendor = join(root, '../../crates/emulator-cpu-unicorn/vendor/unicorn');
  mkdirSync(licenseDirectory, { recursive: true });
  for (const name of unicornLicenses) {
    const fileName = name.replaceAll('/', '-');
    copyFileSync(join(vendor, name), join(licenseDirectory, fileName));
  }
}

function stagePackage(out: string): void {
  const destination = join(root, 'dist/emulator');
  mkdirSync(destination, { recursive: true });
  for (const name of ['camera.mjs', 'camera.wasm', 'camera.d.ts', 'build.json', 'NOTICE']) {
    copyFileSync(join(out, name), join(destination, name));
  }
  const licenseDirectory = join(destination, 'licenses');
  mkdirSync(licenseDirectory, { recursive: true });
  for (const name of unicornLicenses) {
    const fileName = name.replaceAll('/', '-');
    copyFileSync(join(out, 'licenses', fileName), join(licenseDirectory, fileName));
  }
}

export function build(selected: BuildOptions) {
  validateSources();
  const pinned = pinnedToolchain();
  for (const tool of [
    'emcc',
    'emcmake',
    'cmake',
    'ninja',
    'cargo',
    'rustc',
    'rustup',
    'node',
    'git',
  ])
    requireTool(tool);
  try {
    accessSync(wasmBindgen(process.env), constants.X_OK);
  } catch {
    throw new Error('wasm-bindgen is required; run `npm run setup:tooling`.');
  }
  validateToolchain(pinned);

  const plan = buildPlan(selected, process.env);
  const out = resolve(selected.out);
  mkdirSync(out, { recursive: true });
  const unicornSource = join(root, '../../crates/emulator-cpu-unicorn/vendor/unicorn');
  const cache = join(out, 'unicorn/CMakeCache.txt');
  if (existsSync(cache)) {
    const configured = readFileSync(cache, 'utf8').match(
      /^CMAKE_HOME_DIRECTORY:INTERNAL=(.*)$/m,
    )?.[1];
    if (configured && resolve(configured) !== resolve(unicornSource))
      rmSync(join(out, 'unicorn'), { recursive: true, force: true });
  }

  run(
    [process.execPath, join(root, 'jit/build.mts'), join(root, 'jit'), join(out, 'jit')],
    process.env,
  );
  const start = performance.now();
  run(plan.cmakeConfigure, plan.env);
  run(plan.cmakeBuild, plan.env);
  const unicornSeconds = (performance.now() - start) / 1000;
  run(['cargo', `+${pinned.nightly}`, ...plan.rust.slice(1)], plan.env);
  const rustSeconds = (performance.now() - start) / 1000 - unicornSeconds;
  const archive = join(out, 'rust/wasm32-unknown-emscripten/dist/libfujimod_emulator_javascript.a');
  const symbols = capture([
    'emnm',
    '--defined-only',
    '--extern-only',
    '--format=just-symbols',
    '--print-file-name',
    '--quiet',
    archive,
  ]);
  writeFileSync(join(out, 'exports.json'), JSON.stringify(rustExports(symbols)));
  run(plan.link, plan.env);
  const loaderPath = join(out, 'camera.mjs');
  writeFileSync(loaderPath, sharedStringEncoding(readFileSync(loaderPath, 'utf8')));
  const declarations = join(out, 'camera.d.ts');
  const generated = hoistBindgenDeclarations(readFileSync(declarations, 'utf8'));
  writeFileSync(declarations, generated);
  // Checked in so TypeScript consumers type-check without an Emscripten build.
  const checkedIn = join(root, 'emulator/camera.generated.ts');
  const module = declarationModule(generated);
  if (!existsSync(checkedIn) || readFileSync(checkedIn, 'utf8') !== module) {
    writeFileSync(checkedIn, module);
    console.log(`Updated ${checkedIn}; commit it with the Rust API change.`);
  }
  const totalSeconds = (performance.now() - start) / 1000;
  const wasm = readFileSync(join(out, 'camera.wasm'));
  const loader = readFileSync(join(out, 'camera.mjs'));
  const hash = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
  const vendor = join(root, '../../crates/emulator-cpu-unicorn/vendor/unicorn');
  const dirty = capture(['git', 'status', '--porcelain'], vendor).trim();
  if (dirty) throw new Error('The pinned Unicorn source has local changes.');
  const receipt = {
    unicorn_seconds: unicornSeconds,
    rust_seconds: rustSeconds,
    link_seconds: totalSeconds - unicornSeconds - rustSeconds,
    total_seconds: totalSeconds,
    wasm_bytes: wasm.length,
    wasm_sha256: hash(wasm),
    wasm_gzip_bytes: gzipSync(wasm, { level: 9 }).length,
    loader_bytes: loader.length,
    loader_sha256: hash(loader),
    rust: pinned.nightly,
    emscripten: pinned.emsdk,
    profiling: selected.profiling,
    rust_profile: {
      lto: plan.env.CARGO_PROFILE_RELEASE_LTO ?? 'thin',
      codegen_units: Number(plan.env.CARGO_PROFILE_RELEASE_CODEGEN_UNITS ?? '1'),
      opt_level: plan.env.CARGO_PROFILE_RELEASE_OPT_LEVEL ?? '3',
    },
    rust_flags: plan.rustFlags,
    c_flags: plan.cFlags,
    allocator: selected.allocator,
    simd128: { rust: selected.rustSimd128, c: selected.cSimd128 },
    unicorn_commit: capture(['git', 'rev-parse', 'HEAD'], vendor).trim(),
    unicorn_dirty: false,
    unicorn_tci_sha256: hash(readFileSync(join(vendor, 'qemu/tcg/tci.c'))),
  };
  const receiptPath = join(out, 'build.json');
  writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  stageRuntimeNotices(out);
  if (selected.stagePackage) stagePackage(out);
  console.log(JSON.stringify(receipt, null, 2));
  return receipt;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  if (args.includes('--help') || args.includes('-h')) {
    console.log(
      'Usage: node emulator/build.mts [--out DIR] [--stage-package] [--profiling] [--rust-simd128] [--c-simd128] [--lto thin|fat|off] [--opt-level 0|1|2|3|s|z] [--codegen-units N] [--allocator dlmalloc|emmalloc]',
    );
  } else {
    build(options(args));
  }
}
