import { zipBlob } from '@fujimod/emulator-ui/archive';
import { sha256 } from '../../patch/patcher';
import { gitRepository, type GitFile } from './git';
import type { PlannedFile } from './plan';

export interface CardBuildOptions {
  /** Absolute `/releases/<tag>/` URL. */
  assetBase: string;
  release: string;
  git: boolean;
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

/** Fetch and check every planned file, add the Git repository if asked, and zip the card's root. */
export async function buildCard(files: readonly PlannedFile[], options: CardBuildOptions) {
  const fetcher = options.fetcher ?? fetch;
  const written: GitFile[] = [];
  let done = 0;
  for (const file of files) {
    const data =
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
    written.push({ path: file.path, data });
    options.progress?.(++done, files.length);
  }
  const entries = written.map((file) => ({ path: file.path, data: new Blob([file.data.slice()]) }));
  if (options.git) {
    const tracked = written
      .filter((file) => file.path.startsWith('FUJIMOD/'))
      .map((file) => ({ path: file.path.slice('FUJIMOD/'.length), data: file.data }));
    const repository = await gitRepository(tracked, {
      name: 'FujiMod',
      email: 'noreply@fujimod.com',
      message: `FujiMod ${options.release}`,
      time: Date.now() / 1000,
    });
    for (const file of repository)
      entries.push({ path: `FUJIMOD/.git/${file.path}`, data: new Blob([file.data.slice()]) });
  }
  return zipBlob(entries);
}
