/** Stage generated website assets locally before their direct Cloudflare upload. */
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { validateCatalog, type Catalog } from '../src/catalog.ts';
import { validateFeatureMedia, type FeatureMedia } from '../src/home/feature-media.ts';
import { validateRuntimeApi } from './runtime-api.ts';

export interface SiteInputs {
  runtime: string;
  media: string;
}
/** Files of the media for cards the homepage still shows; recordings of removed cards stay behind. */
export function featureMediaFiles(media: FeatureMedia, shownCards: Iterable<string>) {
  const shown = new Set(shownCards);
  const cards = Object.keys(media.cards).filter((card) => shown.has(card));
  return new Set(cards.flatMap((card) => [media.cards[card].video, media.cards[card].still]));
}

/** Cold inputs named by prepared manifests accompany the ordinary runtime files. */
export function runtimeBundleFiles(manifest: unknown) {
  const files = ['camera.wasm', 'camera.mjs', 'wasm.json', 'runtime-NOTICES.txt'];
  if (manifest === null || typeof manifest !== 'object' || !('construction' in manifest))
    throw Error('Runtime manifest has no construction kind.');
  const dmpCpuRenderer = Reflect.get(manifest, 'dmpCpuRenderer');
  if (dmpCpuRenderer !== undefined && typeof dmpCpuRenderer !== 'boolean')
    throw Error('Runtime manifest has an invalid DMP renderer selection.');
  if (dmpCpuRenderer === false)
    files.push('dmp_webgpu.js', 'dmp_webgpu_bg.wasm');
  if (manifest.construction !== 'prepared') return files;
  if (
    !('prepared' in manifest) ||
    manifest.prepared === null ||
    typeof manifest.prepared !== 'object'
  )
    throw Error('Prepared runtime has no bundle files.');
  for (const key of ['document', 'bootRom', 'nand', 'settings']) {
    const name: unknown = Reflect.get(manifest.prepared, key);
    if ((key === 'nand' || key === 'settings') && name === undefined) continue;
    if (typeof name !== 'string' || !/^[a-zA-Z0-9_-][a-zA-Z0-9_.-]*$/.test(name))
      throw Error(`Prepared runtime has an invalid ${key} filename.`);
    files.push(name);
  }
  const camera: unknown = Reflect.get(manifest.prepared, 'camera');
  if (camera !== undefined) {
    if (camera === null || typeof camera !== 'object')
      throw Error('Prepared runtime has an invalid camera input.');
    for (const [section, name] of Object.entries(camera)) {
      if (typeof name !== 'string' || !/^[a-zA-Z0-9_-][a-zA-Z0-9_.-]*$/.test(name))
        throw Error(`Prepared runtime has an invalid camera ${section} filename.`);
      files.push(name);
    }
  }
  return [...new Set(files)];
}

export async function stageSite(inputs: SiteInputs, destination = 'public') {
  const catalog = validateCatalog(
    JSON.parse(await readFile(join(inputs.runtime, 'catalog.json'), 'utf8')) as Catalog,
  );
  const emulators = catalog.firmwares.filter((entry) => entry.emulate);
  if (!emulators.length) throw Error('Website deployment requires runnable emulators.');
  const target = resolve(destination);
  await mkdir(dirname(target), { recursive: true });
  const temporary = await mkdtemp(join(dirname(target), '.site-assets-'));
  const files: { path: string; bytes: number; sha256: string }[] = [];
  async function copy(source: string, path: string) {
    const data = await readFile(source);
    if (!data.length || data.length > 25 * 1024 * 1024) throw Error(`Invalid asset size: ${path}`);
    const staged = join(temporary, path);
    await mkdir(dirname(staged), { recursive: true });
    await writeFile(staged, data);
    files.push({
      path,
      bytes: data.length,
      sha256: createHash('sha256').update(data).digest('hex'),
    });
  }
  async function copyBundle(directory: string, bundle: string) {
    const manifest: unknown = JSON.parse(await readFile(join(directory, 'wasm.json'), 'utf8'));
    for (const name of runtimeBundleFiles(manifest))
      await copy(join(directory, name), bundle + name);
    validateRuntimeApi(
      await readFile(join(directory, 'camera.mjs'), 'utf8'),
      await readFile(
        new URL('../../../bindings/javascript/emulator/camera.generated.ts', import.meta.url),
        'utf8',
      ),
    );
    const licenses = await readdir(join(directory, 'licenses'), { withFileTypes: true });
    if (!licenses.length || licenses.some((entry) => !entry.isFile()))
      throw Error(`Invalid runtime licenses: ${bundle}`);
    for (const entry of licenses)
      await copy(join(directory, 'licenses', entry.name), bundle + 'licenses/' + entry.name);
  }
  try {
    for (const firmware of emulators) {
      const config = firmware.emulate!;
      if (config.bundle !== `runtime/${firmware.id}/`)
        throw Error(`Unexpected runtime location: ${firmware.id}`);
      const directory = join(inputs.runtime, firmware.id);
      const native = JSON.parse(await readFile(join(directory, 'qualification.json'), 'utf8')) as {
        firmware_sha256?: string;
        execution_mode?: string;
        counts_as_boot?: boolean;
        menu?: boolean;
        failure?: unknown;
      };
      const browser = JSON.parse(
        await readFile(join(directory, 'browser-qualification.json'), 'utf8'),
      ) as {
        sha256?: string;
        passed?: boolean;
      };
      if (
        native.firmware_sha256 !== firmware.sha256 ||
        native.execution_mode !== 'strict' ||
        !native.counts_as_boot ||
        !native.menu ||
        native.failure ||
        browser.sha256 !== firmware.sha256 ||
        browser.passed !== true
      )
        throw Error(`Emulator qualification failed: ${firmware.id}`);
      await copyBundle(directory, config.bundle);
      if (config.usb) {
        const bundle = config.usb.bundle;
        if (bundle !== `runtime/${firmware.id}-usb/`)
          throw Error(`Unexpected USB runtime location: ${firmware.id}`);
        const usb = join(inputs.runtime, `${firmware.id}-usb`);
        await copyBundle(usb, bundle);
      }
    }
    for (const firmware of catalog.firmwares) {
      if (!firmware.service_workflow) continue;
      const path = `runtime/${firmware.id}/service-workflow.generated.json`;
      if (firmware.service_workflow !== path)
        throw Error(`Unexpected service workflow location: ${firmware.id}`);
      await copy(join(inputs.runtime, firmware.id, 'service-workflow.generated.json'), path);
    }
    const catalogBytes = Buffer.from(JSON.stringify(catalog));
    await writeFile(join(temporary, 'runtime/catalog.json'), catalogBytes);
    files.push({
      path: 'runtime/catalog.json',
      bytes: catalogBytes.length,
      sha256: createHash('sha256').update(catalogBytes).digest('hex'),
    });
    const media = validateFeatureMedia(
      JSON.parse(await readFile(join(inputs.media, 'media.json'), 'utf8')),
    );
    const homepage: { cards: Record<string, unknown> } = JSON.parse(
      await readFile(new URL('../src/home/features.generated.json', import.meta.url), 'utf8'),
    );
    const names = featureMediaFiles(media, Object.keys(homepage.cards));
    for (const name of names) await copy(join(inputs.media, name), 'artifacts/docs/' + name);
    await copy(join(inputs.media, 'media.json'), 'artifacts/docs/media.json');
    await mkdir(target, { recursive: true });
    for (const directory of ['runtime', 'artifacts']) {
      await rm(join(target, directory), { recursive: true, force: true });
      await rename(join(temporary, directory), join(target, directory));
    }
    await writeFile(
      join(target, 'site-assets.json'),
      JSON.stringify({ schema: 1, files }, null, 2) + '\n',
    );
    return { emulators: emulators.length, files: files.length };
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
