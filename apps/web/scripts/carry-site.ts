/** Carry a deployed site's staged assets forward, verified against its published manifest. */
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { validateCatalog, type Catalog } from '../src/catalog.ts';
import { validateFeatureMedia } from '../src/home/feature-media.ts';
import { validateRuntimeApi } from './runtime-api.ts';

export interface SiteAsset {
  path: string;
  bytes: number;
  sha256: string;
}
/** Staged paths: runtime bundles, the runtime catalog and feature media. */
const ASSET_PATH =
  /^(?:runtime\/catalog\.json|runtime\/[a-z0-9-]+\/(?:licenses\/)?[a-zA-Z0-9_-][a-zA-Z0-9_.-]*|artifacts\/docs\/[a-zA-Z0-9_-][a-zA-Z0-9_.-]*)$/;

/** The `site-assets.json` that staging writes, checked before any file is fetched. */
export function validateSiteAssets(value: unknown): SiteAsset[] {
  if (value === null || typeof value !== 'object' || Reflect.get(value, 'schema') !== 1)
    throw Error('Site asset manifest has an unknown schema.');
  const files: unknown = Reflect.get(value, 'files');
  if (!Array.isArray(files) || !files.length) throw Error('Site asset manifest lists no files.');
  const seen = new Set<string>();
  return files.map((file: unknown): SiteAsset => {
    if (file === null || typeof file !== 'object') throw Error('Invalid site asset entry.');
    const { path, bytes, sha256 } = file as Record<string, unknown>;
    if (typeof path !== 'string' || !ASSET_PATH.test(path) || path.includes('..'))
      throw Error(`Invalid site asset path: ${String(path)}`);
    if (seen.has(path)) throw Error(`Duplicate site asset: ${path}`);
    seen.add(path);
    if (typeof bytes !== 'number' || !Number.isInteger(bytes) || bytes <= 0)
      throw Error(`Invalid site asset size: ${path}`);
    if (typeof sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(sha256))
      throw Error(`Invalid site asset hash: ${path}`);
    return { path, bytes, sha256 };
  });
}

async function download(url: string): Promise<Buffer> {
  const response = await fetch(url, { headers: { 'cache-control': 'no-cache' } });
  if (!response.ok) throw Error(`${url}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

/**
 * Download every asset `origin` lists in its `site-assets.json`, verify each size and
 * SHA-256, check the runtimes against this checkout's Camera API, and install them in
 * `destination` in the layout staging produces.
 */
export async function carrySite(origin: string, destination = 'public', parallel = 8) {
  const base = new URL(origin);
  const manifestBytes = await download(new URL('site-assets.json', base).href);
  const files = validateSiteAssets(JSON.parse(manifestBytes.toString('utf8')));
  const target = resolve(destination);
  await mkdir(dirname(target), { recursive: true });
  const temporary = await mkdtemp(join(dirname(target), '.site-assets-'));
  try {
    const queue = [...files];
    let failed = false;
    // Every worker stops and settles before the temporary directory is removed.
    const workers = await Promise.allSettled(
      Array.from({ length: Math.min(parallel, queue.length) }, async () => {
        try {
          for (let file = queue.shift(); file && !failed; file = queue.shift()) {
            const data = await download(new URL(file.path, base).href);
            const sha256 = createHash('sha256').update(data).digest('hex');
            if (data.length !== file.bytes || sha256 !== file.sha256)
              throw Error(`Deployed asset differs from its manifest: ${file.path}`);
            const staged = join(temporary, file.path);
            await mkdir(dirname(staged), { recursive: true });
            await writeFile(staged, data);
          }
        } catch (error) {
          failed = true;
          throw error;
        }
      }),
    );
    const failure = workers.find((worker) => worker.status === 'rejected');
    if (failure) throw failure.reason;
    const catalog = validateCatalog(
      JSON.parse(await readFile(join(temporary, 'runtime/catalog.json'), 'utf8')) as Catalog,
    );
    const declarations = await readFile(
      new URL('../../../bindings/javascript/emulator/camera.generated.ts', import.meta.url),
      'utf8',
    );
    const bundles = catalog.firmwares.flatMap((firmware) =>
      firmware.emulate
        ? [firmware.emulate.bundle, ...(firmware.emulate.usb ? [firmware.emulate.usb.bundle] : [])]
        : [],
    );
    if (!bundles.length) throw Error('Website deployment requires runnable emulators.');
    for (const bundle of bundles) {
      if (!files.some((file) => file.path === `${bundle}camera.mjs`))
        throw Error(`Deployed runtime is missing: ${bundle}`);
      validateRuntimeApi(
        await readFile(join(temporary, bundle, 'camera.mjs'), 'utf8'),
        declarations,
      );
    }
    validateFeatureMedia(
      JSON.parse(await readFile(join(temporary, 'artifacts/docs/media.json'), 'utf8')),
    );
    await mkdir(target, { recursive: true });
    for (const directory of ['runtime', 'artifacts']) {
      await rm(join(target, directory), { recursive: true, force: true });
      await rename(join(temporary, directory), join(target, directory));
    }
    await writeFile(join(target, 'site-assets.json'), manifestBytes);
    return { emulators: bundles.length, files: files.length };
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
