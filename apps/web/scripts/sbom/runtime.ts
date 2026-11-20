import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join, posix } from 'node:path';
import { cargoComponents } from './cargo.ts';
import { mergeComponents, type Component } from './bom.ts';
import { emscriptenRuntime, rustStandardLibrary } from './toolchain.ts';
import type { Catalog } from '../../src/catalog.ts';

export interface RuntimeSbom {
  schema: 3;
  wasm: Record<string, string>;
  components: Component[];
}

type WasmModule = 'camera.wasm' | 'dmp_webgpu_bg.wasm';

export interface EmulatorLinks {
  rust: string;
  emscripten: string;
  perf: boolean;
  modules: Record<WasmModule, { sha256: string; linkedDirectories: string[] }>;
}

function runtimeWasm(runtime: string) {
  const catalog: Catalog = JSON.parse(readFileSync(join(runtime, 'catalog.json'), 'utf8'));
  const directories = new Set<string>();
  for (const { emulate } of catalog.firmwares) {
    for (const bundle of [emulate?.bundle, emulate?.usb?.bundle]) {
      if (bundle === undefined) continue;
      if (!/^runtime\/[\w-]+\/$/.test(bundle)) throw Error(`Invalid runtime bundle: ${bundle}`);
      directories.add(bundle.slice('runtime/'.length));
    }
  }
  const wasm: Record<string, string> = {};
  for (const directory of directories) {
    const manifest: { dmpCpuRenderer?: boolean } = JSON.parse(
      readFileSync(join(runtime, directory, 'wasm.json'), 'utf8'),
    );
    const names =
      manifest.dmpCpuRenderer === false ? ['camera.wasm', 'dmp_webgpu_bg.wasm'] : ['camera.wasm'];
    for (const name of names) {
      const path = directory + name;
      wasm[path] = createHash('sha256')
        .update(readFileSync(join(runtime, path)))
        .digest('hex');
    }
  }
  return wasm;
}

export function readRuntimeSbom(runtime: string, receipt: string): RuntimeSbom {
  const bom: RuntimeSbom = JSON.parse(readFileSync(receipt, 'utf8'));
  if (bom.schema !== 3 || !bom.wasm || !Array.isArray(bom.components))
    throw Error('Runtime SBOM has no Wasm digests. Stage a runtime with matching linker evidence.');
  const wasm = runtimeWasm(runtime);
  if (
    Object.keys(wasm).length !== Object.keys(bom.wasm).length ||
    Object.entries(wasm).some(([path, sha]) => bom.wasm[path] !== sha)
  )
    throw Error('Runtime SBOM does not match the staged Wasm assets.');
  return bom;
}

export function runtimeSbom(
  runtime: string,
  root: string,
  camera: Component[],
  links: EmulatorLinks,
): RuntimeSbom {
  const wasm = runtimeWasm(runtime);
  const names = new Set<WasmModule>();
  for (const [path, sha] of Object.entries(wasm)) {
    const name = posix.basename(path) as WasmModule;
    if (links.modules?.[name]?.sha256 !== sha)
      throw Error(`No matching linker evidence for runtime/${path} (SHA-256 ${sha}).`);
    names.add(name);
  }
  if (!names.size) return { schema: 3, wasm, components: camera };
  const emulator = cargoComponents(root, [
    {
      package: 'fujimod-emulator-javascript',
      target: 'wasm32-unknown-emscripten',
      product: 'emulator',
      features: links.perf ? ['perf'] : [],
      linkedDirectories: links.modules['camera.wasm'].linkedDirectories,
    },
    ...(names.has('dmp_webgpu_bg.wasm')
      ? [
          {
            package: 'fujimod-emulator-ip-dmp-webgpu',
            target: 'wasm32-unknown-unknown',
            product: 'emulator' as const,
            linkedDirectories: links.modules['dmp_webgpu_bg.wasm'].linkedDirectories,
          },
        ]
      : []),
  ]);
  const components = mergeComponents([
    ...camera,
    ...emulator,
    rustStandardLibrary(root, 'emulator', links.rust),
    emscriptenRuntime(root, links.emscripten),
  ]);
  return { schema: 3, wasm, components: [...components.values()] };
}
