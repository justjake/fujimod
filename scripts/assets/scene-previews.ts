/** Extract each scene RAF's embedded JPEG preview to build/assets/scenes/<id>.jpg
 * for the credits page, without its metadata. */
import { mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { zstdDecompressSync } from 'node:zlib';

const assets = new URL('../../apps/web/src/engine/assets/', import.meta.url);
export const scenePreviewDirectory = fileURLToPath(
  new URL('../../build/assets/scenes/', import.meta.url),
);

type Scene = { file: string };

/** The RAF's preview JPEG: big-endian offset and length at bytes 84 and 88. */
export function rafPreview(raf: Uint8Array): Uint8Array {
  const view = new DataView(raf.buffer, raf.byteOffset, raf.byteLength);
  if (new TextDecoder().decode(raf.subarray(0, 16)) !== 'FUJIFILMCCD-RAW ')
    throw Error('Not a Fujifilm RAF');
  const offset = view.getUint32(84);
  const length = view.getUint32(88);
  if (offset + length > raf.length) throw Error('RAF preview lies outside the file');
  return withoutMetadata(raf.subarray(offset, offset + length));
}

/** The JPEG without APP1 (EXIF, XMP) segments, which carry camera identity. */
export function withoutMetadata(jpeg: Uint8Array): Uint8Array {
  if (jpeg[0] !== 0xff || jpeg[1] !== 0xd8) throw Error('Not a JPEG');
  const parts: Uint8Array[] = [jpeg.subarray(0, 2)];
  let at = 2;
  while (at + 4 <= jpeg.length) {
    if (jpeg[at] !== 0xff) throw Error('Malformed JPEG segment');
    const marker = jpeg[at + 1]!;
    if (marker === 0xda) {
      parts.push(jpeg.subarray(at));
      break;
    }
    const end = at + 2 + ((jpeg[at + 2]! << 8) | jpeg[at + 3]!);
    if (marker !== 0xe1) parts.push(jpeg.subarray(at, end));
    at = end;
  }
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function fresh(output: string, input: URL) {
  try {
    return (await stat(output)).mtimeMs >= (await stat(input)).mtimeMs;
  } catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return false;
    throw error;
  }
}

export async function prepareScenePreviews(): Promise<string> {
  const scenes: Record<string, Scene> = JSON.parse(
    await readFile(new URL('scenes.json', assets), 'utf8'),
  );
  await mkdir(scenePreviewDirectory, { recursive: true });
  for (const [id, scene] of Object.entries(scenes)) {
    const input = new URL(scene.file, assets);
    const output = `${scenePreviewDirectory}${id}.jpg`;
    if (await fresh(output, input)) continue;
    const temporary = `${output}.${process.pid}.tmp`;
    try {
      await writeFile(temporary, rafPreview(zstdDecompressSync(await readFile(input))));
      await rename(temporary, output);
    } finally {
      await rm(temporary, { force: true });
    }
  }
  return scenePreviewDirectory;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  console.log(await prepareScenePreviews());
