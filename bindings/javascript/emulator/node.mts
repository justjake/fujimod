import { readFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { ModuleOptions } from './camera.generated.ts';
import {
  createCamera,
  instantiateEmulatorModule,
  type Camera,
  type EmulatorModule,
  type EmulatorModuleFactory,
  type PreparedInputs,
} from './module.ts';

/** Load a generated module from disk and its adjacent Wasm binary. */
export async function loadNodeModule(
  modulePath: string,
  options: ModuleOptions = {},
): Promise<EmulatorModule> {
  const absolutePath = resolve(modulePath);
  if (!absolutePath.endsWith('.mjs')) throw new Error('Emulator module path must end in .mjs.');
  const loaded: unknown = await import(pathToFileURL(absolutePath).href);
  if (loaded === null || typeof loaded !== 'object' || !('default' in loaded))
    throw new Error('Generated module has no factory.');
  const factory = loaded.default;
  if (typeof factory !== 'function') throw new Error('Generated module has no factory.');
  const wasmPath = resolve(dirname(absolutePath), basename(absolutePath, '.mjs') + '.wasm');
  const wasmBinary = await readFile(wasmPath);
  const wasmBytes = wasmBinary.buffer.slice(
    wasmBinary.byteOffset,
    wasmBinary.byteOffset + wasmBinary.byteLength,
  );
  return instantiateEmulatorModule(
    factory as EmulatorModuleFactory,
    wasmBytes,
    (path) => pathToFileURL(resolve(dirname(absolutePath), path)).href,
    options,
  );
}

export interface OpenCamera {
  module: EmulatorModule;
  camera: Camera;
}

/** Open an EXR II camera from its generated module, device graph document and DAT. */
export async function openCamera(
  modulePath: string,
  configPath: string,
  datPath: string,
  options: ModuleOptions = {},
): Promise<OpenCamera> {
  const module = await loadNodeModule(modulePath, options);
  const config: unknown = JSON.parse(await readFile(configPath, 'utf8'));
  return { module, camera: createCamera(module, config, await readFile(datPath), 'session') };
}

/**
 * Read a native prepared bundle directory: its manifest names the document,
 * boot ROM and the camera's compressed NAND sections beside it.
 */
export async function readPreparedBundle(
  directory: string,
): Promise<{ document: string; inputs: PreparedInputs }> {
  const manifest: unknown = JSON.parse(await readFile(join(directory, 'wasm.json'), 'utf8'));
  const prepared: unknown =
    manifest !== null && typeof manifest === 'object' ? Reflect.get(manifest, 'prepared') : null;
  if (prepared === null || typeof prepared !== 'object')
    throw new Error('Prepared bundle has no prepared inputs.');
  const file = (name: unknown) => {
    if (typeof name !== 'string' || name !== basename(name))
      throw new Error('Prepared bundle names an invalid input file.');
    return join(directory, name);
  };
  const camera: unknown = Reflect.get(prepared, 'camera') ?? {};
  if (camera === null || typeof camera !== 'object')
    throw new Error('Prepared bundle has an invalid camera input.');
  return {
    document: await readFile(file(Reflect.get(prepared, 'document')), 'utf8'),
    inputs: {
      bootRom: await readFile(file(Reflect.get(prepared, 'bootRom'))),
      camera: Object.fromEntries(
        await Promise.all(
          Object.entries(camera).map(
            async ([section, name]: [string, unknown]) =>
              [section, await readFile(file(name))] as const,
          ),
        ),
      ),
    },
  };
}

/** Open a native prepared run from its bundle directory and the update container. */
export async function openPreparedCamera(
  modulePath: string,
  bundlePath: string,
  updatePath: string,
  options: ModuleOptions = {},
): Promise<OpenCamera> {
  const module = await loadNodeModule(modulePath, options);
  const { document, inputs } = await readPreparedBundle(bundlePath);
  const camera = createCamera(
    module,
    JSON.stringify({ document }),
    await readFile(updatePath),
    'prepared',
    inputs,
  );
  return { module, camera };
}

/** Open an exported running session without separate firmware or graph files. */
export async function openSnapshot(
  modulePath: string,
  snapshotPath: string,
  options: ModuleOptions = {},
): Promise<OpenCamera> {
  const module = await loadNodeModule(modulePath, options);
  return { module, camera: module.Camera.fromSnapshot(await readFile(snapshotPath)) };
}
