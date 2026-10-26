interface AssetMetadata {
  etag: string;
  contentType: string;
}
interface StoredAsset extends AssetMetadata {
  bytes: ArrayBuffer | Blob;
}
interface AssetStore {
  read(url: string): Promise<StoredAsset | undefined>;
  write(url: string, metadata: AssetMetadata, response: Response): Promise<void>;
}
async function transaction(action: 'get' | 'put', url: string, value?: StoredAsset) {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open('fujimod-assets-v1', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('files');
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
  try {
    return await new Promise<StoredAsset | undefined>((resolve, reject) => {
      const tx = db.transaction('files', action === 'get' ? 'readonly' : 'readwrite');
      const store = tx.objectStore('files');
      const request = action === 'get' ? store.get(url) : store.put(value, url);
      tx.oncomplete = () => resolve(action === 'get' ? request.result : undefined);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}
async function directory() {
  return (await navigator.storage.getDirectory()).getDirectoryHandle('fujimod-assets-v1', {
    create: true,
  });
}
async function key(url: string) {
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(url));
  return Array.from(new Uint8Array(hash), (value) => value.toString(16).padStart(2, '0')).join('');
}
const disk: AssetStore = {
  async read(url) {
    try {
      const file = await (await (await directory()).getFileHandle(await key(url))).getFile();
      const head = new Uint8Array(await file.slice(0, 4096).arrayBuffer()),
        end = head.indexOf(10);
      if (end < 0) throw Error('Asset cache metadata is incomplete');
      const metadata = JSON.parse(new TextDecoder().decode(head.subarray(0, end)));
      if (typeof metadata.etag !== 'string' || typeof metadata.contentType !== 'string')
        throw Error('Invalid asset metadata');
      return { ...metadata, bytes: file.slice(end + 1) };
    } catch {
      return transaction('get', url);
    }
  },
  async write(url, metadata, response) {
    let dir;
    try {
      dir = await directory();
    } catch {
      await transaction('put', url, { ...metadata, bytes: await response.arrayBuffer() });
      return;
    }
    const file = await dir.getFileHandle(await key(url), { create: true });
    const writer = await file.createWritable();
    try {
      await writer.write(new TextEncoder().encode(JSON.stringify(metadata) + '\n'));
      // Stream into the atomic replacement file. Large DAT/package responses
      // need no Blob or structured-clone copy in WebKit's networking process.
      if (response.body) {
        const reader = response.body.getReader();
        try {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            await writer.write(value);
          }
        } finally {
          reader.releaseLock();
        }
      }
      await writer.close();
    } catch (error) {
      await writer.abort().catch(() => {});
      throw error;
    }
  },
};
/** Revalidate large artifacts against their source ETag. OPFS streams large
 * responses; IndexedDB supports private contexts without OPFS. Integrity checks
 * remain with the caller (DAT catalog, runtime or package). */
export async function cachedFetch(
  url: string,
  fetcher: typeof fetch = fetch,
  storage: AssetStore = disk,
): Promise<Response> {
  try {
    const saved = await storage.read(url);
    if (saved) {
      const current = await fetcher(url, { method: 'HEAD', cache: 'no-store' });
      if (current.ok && current.headers.get('etag') === saved.etag)
        return new Response(saved.bytes, {
          headers: {
            etag: saved.etag,
            'content-type': saved.contentType,
            'content-length': String(
              saved.bytes instanceof Blob ? saved.bytes.size : saved.bytes.byteLength,
            ),
          },
        });
    }
  } catch {
    /* Unavailable storage falls through to the ordinary local fetch. */
  }
  const response = await fetcher(url, { cache: 'no-store' });
  const etag = response.headers.get('etag');
  if (response.ok && etag) {
    void storage
      .write(
        url,
        { etag, contentType: response.headers.get('content-type') || 'application/octet-stream' },
        response.clone(),
      )
      .catch(() => {});
  }
  return response;
}
