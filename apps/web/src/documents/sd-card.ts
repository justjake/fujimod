/** Folder access to an SD card through the File System Access API (Chrome and
 * Edge on computers). The granted folder is remembered for every editor page
 * on this site; a later visit asks again with one tap. */

type Mode = { mode: 'readwrite' };
type Permissioned = FileSystemDirectoryHandle & {
  queryPermission?(options: Mode): Promise<PermissionState>;
  requestPermission?(options: Mode): Promise<PermissionState>;
  values(): AsyncIterable<FileSystemHandle>;
};
type Picker = (options: { id?: string; mode?: 'readwrite' }) => Promise<FileSystemDirectoryHandle>;

const DATABASE = 'fujimod-sd-card';
const STORE = 'handles';
const KEY = 'root';
const readwrite: Mode = { mode: 'readwrite' };

function picker(): Picker | undefined {
  return typeof window === 'undefined'
    ? undefined
    : (window as Window & { showDirectoryPicker?: Picker }).showDirectoryPicker;
}

/** Whether this browser can edit files on an SD card in place. */
export function folderAccessSupported() {
  return !!picker();
}

function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function stored<T>(
  action: (store: IDBObjectStore) => IDBRequest<T>,
  mode: IDBTransactionMode,
) {
  const db = await database();
  try {
    return await new Promise<T>((resolve, reject) => {
      const request = action(db.transaction(STORE, mode).objectStore(STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  } finally {
    db.close();
  }
}

export type SdCard = { root: Permissioned; granted: boolean };

/** The remembered SD card folder, if any, and whether it may be edited now. */
export async function rememberedSdCard(): Promise<SdCard | undefined> {
  if (!folderAccessSupported()) return undefined;
  const root = await stored<Permissioned | undefined>((store) => store.get(KEY), 'readonly');
  if (!root) return undefined;
  return { root, granted: (await root.queryPermission?.(readwrite)) === 'granted' };
}

/** Ask for the remembered folder again; needs a tap or click. */
export async function regrant(card: SdCard): Promise<SdCard> {
  const state = await card.root.requestPermission?.(readwrite);
  return { root: card.root, granted: state === 'granted' };
}

/** Choose the SD card (or its FUJIMOD folder); needs a tap or click. */
export async function chooseSdCard(): Promise<SdCard> {
  const choose = picker();
  if (!choose) throw Error('This browser cannot edit files on an SD card.');
  const root = (await choose({ id: 'fujimod-sd-card', mode: 'readwrite' })) as Permissioned;
  await stored((store) => store.put(root, KEY), 'readwrite');
  return { root, granted: true };
}

export async function forgetSdCard() {
  await stored((store) => store.delete(KEY), 'readwrite');
}

/** The folder at a path from the SD card's root. A chosen folder named like
 * the path's first part (FUJIMOD) is taken to be that folder. */
async function folder(card: SdCard, path: string, create: boolean) {
  const parts = path.split('/').filter(Boolean);
  if (parts[0] && parts[0].toUpperCase() === card.root.name.toUpperCase()) parts.shift();
  let current: FileSystemDirectoryHandle = card.root;
  for (const part of parts) current = await current.getDirectoryHandle(part, { create });
  return current;
}

const missing = (error: unknown) => error instanceof DOMException && error.name === 'NotFoundError';

/** File names in a folder with one of the extensions, sorted; none if the folder is missing. */
export async function listFiles(card: SdCard, directory: string, extensions: readonly string[]) {
  let dir: Permissioned;
  try {
    dir = (await folder(card, directory, false)) as Permissioned;
  } catch (error) {
    if (missing(error)) return [];
    throw error;
  }
  const wanted = extensions.map((extension) => extension.toUpperCase());
  const names: string[] = [];
  for await (const entry of dir.values())
    if (
      entry.kind === 'file' &&
      wanted.some((extension) => entry.name.toUpperCase().endsWith(extension))
    )
      names.push(entry.name);
  return names.sort((a, b) => a.localeCompare(b));
}

/** A file's text, or undefined if it does not exist. */
export async function readText(card: SdCard, directory: string, name: string) {
  try {
    const dir = await folder(card, directory, false);
    return await (await (await dir.getFileHandle(name)).getFile()).text();
  } catch (error) {
    if (missing(error)) return undefined;
    throw error;
  }
}

export async function writeText(card: SdCard, directory: string, name: string, text: string) {
  const dir = await folder(card, directory, true);
  const writer = await (await dir.getFileHandle(name, { create: true })).createWritable();
  try {
    await writer.write(text);
    await writer.close();
  } catch (error) {
    await writer.abort().catch(() => {});
    throw error;
  }
}

export async function removeFile(card: SdCard, directory: string, name: string) {
  await (await folder(card, directory, false)).removeEntry(name);
}
