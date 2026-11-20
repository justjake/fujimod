import { zipBlob } from '@fujimod/emulator-ui/archive';
import { sha256 } from '../../patch/patcher';
import type { PlannedFile } from './plan';

export interface CardBuildOptions {
  /** Absolute `/releases/<tag>/` URL. */
  assetBase: string;
  fetcher?: typeof fetch;
  progress?: (done: number, total: number) => void;
}

async function fetchChecked(url: URL, sha: string, bytes: number, fetcher: typeof fetch) {
  const response = await fetcher(url);
  if (!response.ok) throw Error(`Download failed: ${url.pathname}`);
  const data = new Uint8Array(await response.arrayBuffer());
  if (data.length !== bytes || (await sha256(data)) !== sha)
    throw Error(`${url.pathname} failed its integrity check.`);
  return data;
}

/** Fetch and check every planned file: the SD card's root-relative paths and their bytes. */
export async function cardFiles(files: readonly PlannedFile[], options: CardBuildOptions) {
  const fetcher = options.fetcher ?? fetch;
  const entries: { path: string; bytes: Uint8Array }[] = [];
  let done = 0;
  for (const file of files) {
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
          );
    entries.push({ path: file.path, bytes });
    options.progress?.(++done, files.length);
  }
  return entries;
}

/** Fetch and check every planned file, and zip the SD card's root. */
export async function buildCard(files: readonly PlannedFile[], options: CardBuildOptions) {
  const entries = await cardFiles(files, options);
  return zipBlob(entries.map(({ path, bytes }) => ({ path, data: new Blob([bytes.slice()]) })));
}
