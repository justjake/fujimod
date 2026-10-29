import { validateIndex, type PatchIndex } from './catalog.ts';
import { validateReleaseInfo, type ReleaseInfo } from './releases.ts';

export interface LoadedRelease extends PatchIndex, ReleaseInfo {
  /** Absolute same-origin /releases/<tag>/ URL, also passed to the patch worker. */
  assetBase: string;
  /** Release Markdown, read directly from editable source during development. */
  notesText?: string;
}
/** Fetch release metadata and Markdown; a missing release is normal. */
export async function loadRelease(
  base: string | URL,
  fetcher: typeof fetch = fetch,
): Promise<LoadedRelease | undefined> {
  const root = new URL('/releases/', base);
  const response = await fetcher(new URL('current', root));
  if (response.status === 404) return;
  if (!response.ok) throw Error('Current release unavailable.');
  const info = validateReleaseInfo(await response.json());
  const assetBase = new URL(`${encodeURIComponent(info.tag)}/`, root).href;
  async function asset(name: string) {
    const response = await fetcher(new URL(name, assetBase));
    if (!response.ok) throw Error(`Release asset unavailable: ${name}`);
    return response;
  }
  const index = validateIndex(await (await asset('patches.json')).json());
  if (index.release !== info.tag) throw Error('The release index does not match its tag.');
  const notesText = index.notes ? await (await asset(index.notes.name)).text() : undefined;
  return { ...index, ...info, assetBase, ...(notesText === undefined ? {} : { notesText }) };
}
