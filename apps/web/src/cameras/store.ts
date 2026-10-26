import {
  writeDurable,
  readDurable,
  removeDurable,
  type DurableReference,
} from '../storage/durable';
import type { DevicePanel } from '@fujimod/emulator-ui/ui/layout';
/** One official firmware update in the browser, shared by every camera made from it. */
export interface LibraryFirmware {
  sha256: string;
  firmwareId: string;
  name: string;
  fileName: string;
  addedAt: number;
}
/** Owned cameras are independent of the disposable firmware boot cache. */
export interface CameraRecord {
  id: string;
  name: string;
  firmwareId: string;
  firmwareHash: string;
  fileName: string;
  createdAt: number;
  savedAt?: number;
  poweredOn?: boolean;
  revision: number;
  modEnabled: boolean;
  layout?: DevicePanel;
}
export interface CameraSave {
  durable: ArrayBuffer;
  runtimeKey: string;
  poweredOn: boolean;
  positions: Record<string, string>;
  modEnabled: boolean;
}
interface StoredSave extends Omit<CameraSave, 'durable'> {
  schema: 1;
  durable: DurableReference;
}
const DB = 'fujimod-cameras';
/** Version 1 kept a firmware copy per camera; version 2 keeps one per SHA-256. */
function upgrade(request: IDBOpenDBRequest, oldVersion: number) {
  const db = request.result,
    tx = request.transaction!;
  if (oldVersion < 1) {
    db.createObjectStore('cameras', { keyPath: 'id' });
    db.createObjectStore('saves');
  }
  db.createObjectStore('library', { keyPath: 'sha256' });
  db.createObjectStore('library-bytes');
  if (oldVersion < 1) return;
  const library = tx.objectStore('library'),
    bytes = tx.objectStore('library-bytes'),
    copies = tx.objectStore('firmware');
  const cursor = tx.objectStore('cameras').openCursor();
  cursor.onsuccess = () => {
    const position = cursor.result;
    if (!position) {
      db.deleteObjectStore('firmware');
      return;
    }
    const camera = position.value as CameraRecord;
    const copy = copies.get(camera.id);
    copy.onsuccess = () => {
      if (copy.result !== undefined) {
        const firmware: LibraryFirmware = {
          sha256: camera.firmwareHash,
          firmwareId: camera.firmwareId,
          name: camera.name,
          fileName: camera.fileName,
          addedAt: camera.createdAt,
        };
        library.put(firmware);
        bytes.put(copy.result, camera.firmwareHash);
      }
      position.continue();
    };
  };
}
async function database() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB, 2);
    request.onupgradeneeded = (event) => upgrade(request, event.oldVersion);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
/** Add `firmware` to the library in `tx` unless it is already there. */
function addToLibrary(
  tx: IDBTransaction,
  firmware: LibraryFirmware,
  bytes: ArrayBuffer,
  fail: (error: unknown) => void,
) {
  const library = tx.objectStore('library');
  const existing = library.getKey(firmware.sha256);
  existing.onsuccess = () => {
    if (existing.result !== undefined) return;
    try {
      library.add(firmware);
      tx.objectStore('library-bytes').add(bytes, firmware.sha256);
    } catch (error) {
      fail(error);
    }
  };
}
async function transaction<T>(
  stores: string[],
  mode: IDBTransactionMode,
  run: (tx: IDBTransaction, result: (value: T) => void, fail: (error: unknown) => void) => void,
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await transactionAttempt(stores, mode, run);
    } catch (error) {
      // WebKit can transiently abort an initial open/read. Reopen once for
      // read-only operations; never replay a write or hide persistent failures.
      if (
        mode !== 'readonly' ||
        attempt > 0 ||
        !(error instanceof DOMException) ||
        error.name !== 'AbortError'
      )
        throw error;
    }
  }
}
async function transactionAttempt<T>(
  stores: string[],
  mode: IDBTransactionMode,
  run: (tx: IDBTransaction, result: (value: T) => void, fail: (error: unknown) => void) => void,
): Promise<T> {
  const db = await database();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(stores, mode);
      let value: T;
      let failure: DOMException | null = null;
      tx.oncomplete = () => resolve(value);
      tx.onerror = (event) => {
        failure = (event.target as IDBRequest).error;
      };
      tx.onabort = () => reject(tx.error ?? failure ?? Error('Camera storage transaction failed.'));
      const fail = (error: unknown) => {
        try {
          tx.abort();
        } catch {
          // A browser may already have aborted the transaction.
        }
        reject(error);
      };
      try {
        run(
          tx,
          (result) => {
            value = result;
          },
          fail,
        );
      } catch (error) {
        fail(error);
      }
    });
  } finally {
    db.close();
  }
}
/** A firmware update identified against the catalog. */
export interface IdentifiedFirmware {
  id: string;
  model: string;
  version: string;
  sha256: string;
}
function libraryEntry(file: File, firmware: IdentifiedFirmware): LibraryFirmware {
  return {
    sha256: firmware.sha256,
    firmwareId: firmware.id,
    name: `${firmware.model} · ${firmware.version}`,
    fileName: file.name,
    addedAt: Date.now(),
  };
}
export class CameraStore {
  list(): Promise<CameraRecord[]> {
    return transaction(['cameras'], 'readonly', (tx, result) => {
      const request = tx.objectStore('cameras').getAll();
      request.onsuccess = () =>
        result((request.result as CameraRecord[]).sort((a, b) => b.createdAt - a.createdAt));
    });
  }
  /** Stored firmware, most recently added first. */
  library(): Promise<LibraryFirmware[]> {
    return transaction(['library'], 'readonly', (tx, result) => {
      const request = tx.objectStore('library').getAll();
      request.onsuccess = () =>
        result((request.result as LibraryFirmware[]).sort((a, b) => b.addedAt - a.addedAt));
    });
  }
  /** Store an identified firmware update once; a repeat returns the stored entry. */
  async addFirmware(file: File, firmware: IdentifiedFirmware): Promise<LibraryFirmware> {
    // Store bytes rather than File/Blob handles: WebKit can fail their IndexedDB clone.
    const bytes = await file.arrayBuffer();
    const entry = libraryEntry(file, firmware);
    return transaction(['library', 'library-bytes'], 'readwrite', (tx, result, fail) => {
      const existing = tx.objectStore('library').get(entry.sha256);
      existing.onsuccess = () => {
        const stored = existing.result as LibraryFirmware | undefined;
        result(stored ?? entry);
      };
      addToLibrary(tx, entry, bytes, fail);
    });
  }
  async create(
    file: File,
    firmware: IdentifiedFirmware,
    modEnabled: boolean,
  ): Promise<CameraRecord> {
    // Store bytes rather than File/Blob handles: WebKit can fail their IndexedDB clone.
    const bytes = await file.arrayBuffer();
    const record: CameraRecord = {
      id: crypto.randomUUID(),
      name: `${firmware.model} · ${firmware.version}`,
      firmwareId: firmware.id,
      firmwareHash: firmware.sha256,
      fileName: file.name,
      createdAt: Date.now(),
      revision: 0,
      modEnabled,
    };
    await transaction(['cameras', 'library', 'library-bytes'], 'readwrite', (tx, result, fail) => {
      tx.objectStore('cameras').add(record);
      addToLibrary(tx, libraryEntry(file, firmware), bytes, fail);
      result(undefined);
    });
    return record;
  }
  /** The firmware update with this SHA-256. */
  firmware(sha256: string): Promise<Blob> {
    return transaction(['library-bytes'], 'readonly', (tx, result) => {
      const request = tx.objectStore('library-bytes').get(sha256);
      request.onsuccess = () => result(request.result);
    }).then((value) => {
      if (!(value instanceof ArrayBuffer) && !(value instanceof Blob))
        throw Error('This firmware is missing from browser storage.');
      return new Blob([value]);
    });
  }
  async saved(id: string): Promise<CameraSave | undefined> {
    for (let attempt = 0; ; attempt++) {
      const saved = await transaction<StoredSave | undefined>(
        ['saves'],
        'readonly',
        (tx, result) => {
          const request = tx.objectStore('saves').get(id);
          request.onsuccess = () => result(request.result);
        },
      );
      if (!saved) return undefined;
      if (saved.schema !== 1) throw Error('Unknown durable camera save format.');
      try {
        const { schema: _, durable, ...metadata } = saved;
        return { ...metadata, durable: await readDurable(durable) };
      } catch (error) {
        if (attempt === 0 && error instanceof DOMException && error.name === 'NotFoundError')
          continue;
        throw error;
      }
    }
  }
  /** A closed immutable file becomes current in the same optimistic metadata transaction. */
  async save(id: string, revision: number, save: CameraSave): Promise<CameraRecord> {
    const durable = await writeDurable(save.durable);
    let published: { record: CameraRecord; previous?: StoredSave } | undefined;
    try {
      published = await transaction(['cameras', 'saves'], 'readwrite', (tx, result, fail) => {
        const cameras = tx.objectStore('cameras'),
          saves = tx.objectStore('saves');
        const request = cameras.get(id);
        request.onsuccess = () => {
          const previous = request.result as CameraRecord | undefined;
          if (!previous || previous.revision !== revision) {
            result(undefined);
            return;
          }
          const old = saves.get(id);
          old.onsuccess = () => {
            try {
              const record = {
                ...previous,
                savedAt: Date.now(),
                poweredOn: save.poweredOn,
                modEnabled: save.modEnabled,
                revision: revision + 1,
              };
              saves.put({ ...save, schema: 1, durable } satisfies StoredSave, id);
              cameras.put(record);
              result({ record, previous: old.result });
            } catch (error) {
              fail(error);
            }
          };
        };
      });
      if (!published) throw Error('This camera changed in another tab. Reload it before saving.');
    } catch (error) {
      await removeDurable(durable).catch(() => {});
      throw error;
    }
    if (published.previous?.schema === 1)
      await removeDurable(published.previous.durable).catch(() => {});
    return published.record;
  }
  saveLayout(id: string, layout: DevicePanel): Promise<CameraRecord> {
    return transaction(['cameras'], 'readwrite', (tx, result, fail) => {
      const cameras = tx.objectStore('cameras');
      const request = cameras.get(id);
      request.onsuccess = () => {
        const record = request.result as CameraRecord | undefined;
        if (!record) {
          fail(Error('This camera is no longer in browser storage.'));
          return;
        }
        const updated = { ...record, layout };
        cameras.put(updated);
        result(updated);
      };
    });
  }
}
