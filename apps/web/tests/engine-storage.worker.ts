/** Browser storage checks run on a real dedicated worker so OPFS uses its native API. */
import { CameraStore, type CameraRecord, type CameraSave } from '../src/cameras/store';
import type { DurableReference } from '../src/storage/durable';
export interface StorageResult {
  id: string;
  first: CameraRecord;
  second: CameraRecord;
  one: { durable: DurableReference };
  two: { durable: DurableReference };
  after: { durable: DurableReference };
  stale: string;
  quota: string;
  retained: number[];
  files: string[];
  bytes: number[];
}
export interface ResumeResult {
  bytes: number[];
  record: CameraRecord | undefined;
}
export type StorageReply = { result: StorageResult | ResumeResult } | { error: string };
async function listing(root: FileSystemDirectoryHandle) {
  const names: string[] = [];
  for await (const [name] of root.entries()) names.push(name);
  return names;
}
async function exercise(): Promise<StorageResult> {
  const store = new CameraStore();
  const root = await (
    await navigator.storage.getDirectory()
  ).getDirectoryHandle('camera-media', { create: true });
  const before = await listing(root);
  const camera = await store.create(
    new File([new Uint8Array([1, 2, 3])], 'test.dat'),
    { id: 'storage', model: 'Storage', version: '1', sha256: 'test' },
    false,
  );
  const save = (bytes: number[]): CameraSave => ({
    durable: new Uint8Array(bytes).buffer,
    runtimeKey: 'storage-test',
    poweredOn: true,
    positions: {},
    modEnabled: false,
  });
  const descriptor = () =>
    new Promise<{ durable: DurableReference }>((resolve, reject) => {
      const request = indexedDB.open('fujimod-cameras', 1);
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result,
          tx = db.transaction('saves'),
          get = tx.objectStore('saves').get(camera.id);
        get.onerror = () => reject(get.error);
        get.onsuccess = () => resolve(get.result as { durable: DurableReference });
        tx.oncomplete = () => db.close();
      };
    });
  const first = await store.save(camera.id, 0, save([4, 5, 6])),
    one = await descriptor();
  let stale = '';
  try {
    await store.save(camera.id, 0, save([9]));
  } catch (error) {
    stale = String(error);
  }
  const retained = [...new Uint8Array((await store.saved(camera.id))!.durable)];
  const second = await store.save(camera.id, first.revision, save([7, 8])),
    two = await descriptor();
  const original = IDBObjectStore.prototype.put;
  let quota = '';
  try {
    IDBObjectStore.prototype.put = function (...args: Parameters<IDBObjectStore['put']>) {
      if (this.name === 'cameras')
        throw new DOMException('forced metadata quota', 'QuotaExceededError');
      return original.apply(this, args);
    };
    await store.save(camera.id, second.revision, save([10]));
  } catch (error) {
    quota = String(error);
  } finally {
    IDBObjectStore.prototype.put = original;
  }
  return {
    id: camera.id,
    first,
    second,
    one,
    two,
    after: await descriptor(),
    stale,
    quota,
    retained,
    files: (await listing(root)).filter((name) => !before.includes(name)),
    bytes: [...new Uint8Array((await store.saved(camera.id))!.durable)],
  };
}
onmessage = async ({ data }: MessageEvent<{ id?: string }>) => {
  try {
    const store = new CameraStore();
    const result = data.id
      ? {
          bytes: [...new Uint8Array((await store.saved(data.id))!.durable)],
          record: (await store.list()).find((camera) => camera.id === data.id),
        }
      : await exercise();
    postMessage({ result } satisfies StorageReply);
  } catch (error) {
    postMessage({ error: String(error) } satisfies StorageReply);
  }
};
