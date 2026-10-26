/** Materialize the single, hash-verified RAF shared by every default-scene consumer,
 * and its gzip copy for the website (Cloudflare assets are limited to 25 MiB). */
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, writeFile, rm } from 'node:fs/promises';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gunzipSync, gzipSync, zstdDecompressSync } from 'node:zlib';

export const originalSha256 = '2fdbda765e9abf147d4bdfdd5a2060d2c8702d7ad53561a5bde664bc50d53cf4';
export const defaultSceneRaf = fileURLToPath(
  new URL('../../build/assets/default-scene.raf', import.meta.url),
);
export const defaultSceneRafGzip = `${defaultSceneRaf}.gzipdata`;
const original = new URL('../../apps/web/src/engine/assets/default-scene.raf.zst', import.meta.url);
const hash = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');

async function current(path: string, decode: (bytes: Buffer) => Uint8Array) {
  try {
    return hash(decode(await readFile(path))) === originalSha256;
  } catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return false;
    if (error instanceof Error && 'code' in error && error.code === 'Z_DATA_ERROR') return false;
    throw error;
  }
}

async function replace(path: string, bytes: Uint8Array) {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, bytes);
    await rename(temporary, path);
  } finally {
    await rm(temporary, { force: true });
  }
}

export async function prepareDefaultScene(): Promise<string> {
  const raw = await current(defaultSceneRaf, (bytes) => bytes);
  const gzip = await current(defaultSceneRafGzip, (bytes) => gunzipSync(bytes));
  if (raw && gzip) return defaultSceneRaf;
  const bytes = zstdDecompressSync(await readFile(original));
  if (hash(bytes) !== originalSha256)
    throw Error('Default RAF does not match the recorded original SHA-256.');
  if (!raw) await replace(defaultSceneRaf, bytes);
  if (!gzip) await replace(defaultSceneRafGzip, gzipSync(bytes, { level: 9 }));
  return defaultSceneRaf;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  console.log(await prepareDefaultScene());
