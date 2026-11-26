import { zipBlob } from '@fujimod/emulator-ui/archive';
import { sha256 } from '../../patch/patcher';
import type { PlannedFile } from './plan';

export interface CardBuildOptions {
  /** Absolute `/releases/<tag>/` URL. */
  assetBase: string;
  fetcher?: typeof fetch;
  /** Bytes received so far, of the planned total. */
  progress?: (received: number, total: number) => void;
}

/** Files fetched at once. */
const parallel = 6;

async function fetchChecked(
  url: URL,
  sha: string,
  bytes: number,
  fetcher: typeof fetch,
  received: (bytes: number) => void,
) {
  const response = await fetcher(url);
  if (!response.ok || !response.body) throw Error(`Download failed: ${url.pathname}`);
  const data = new Uint8Array(bytes);
  let length = 0;
  for await (const chunk of response.body) {
    if (length + chunk.byteLength > bytes)
      throw Error(`${url.pathname} failed its integrity check.`);
    data.set(chunk, length);
    length += chunk.byteLength;
    received(chunk.byteLength);
  }
  if (length !== bytes || (await sha256(data)) !== sha)
    throw Error(`${url.pathname} failed its integrity check.`);
  return data;
}

/** Fetch and check every planned file: the SD card's root-relative paths and their bytes. */
export async function cardFiles(files: readonly PlannedFile[], options: CardBuildOptions) {
  const fetcher = options.fetcher ?? fetch;
  const total = files.reduce((sum, file) => sum + (file.from === 'data' ? 0 : file.bytes), 0);
  let received = 0;
  const add = (bytes: number) => {
    received += bytes;
    options.progress?.(received, total);
  };
  const entries: { path: string; bytes: Uint8Array }[] = Array.from(files, () => ({
    path: '',
    bytes: new Uint8Array(),
  }));
  let next = 0;
  async function worker() {
    while (next < files.length) {
      const index = next++,
        file = files[index];
      const bytes =
        file.from === 'data'
          ? file.data
          : await fetchChecked(
              new URL(
                encodeURIComponent(file.from === 'release' ? file.asset : file.name),
                options.assetBase,
              ),
              file.sha256,
              file.bytes,
              fetcher,
              add,
            );
      entries[index] = { path: file.path, bytes };
    }
  }
  await Promise.all(Array.from({ length: Math.min(parallel, files.length) }, worker));
  return entries;
}

/** Fetch and check every planned file, and zip the SD card's root. */
export async function buildCard(files: readonly PlannedFile[], options: CardBuildOptions) {
  const entries = await cardFiles(files, options);
  return zipBlob(entries.map(({ path, bytes }) => ({ path, data: new Blob([bytes.slice()]) })));
}
