/** Immutable generations: publish a reference only after the complete file closes. */
export type DurableReference =
  | { backend: 'opfs'; file: string }
  | { backend: 'indexeddb'; data: ArrayBuffer };
const DIRECTORY = 'camera-media';
const MAX_BYTES = 256 * 1024 * 1024;
function unsupported(error: unknown) {
  return (
    error instanceof DOMException &&
    ['NotSupportedError', 'SecurityError', 'NotAllowedError'].includes(error.name)
  );
}
async function directory() {
  return (await navigator.storage.getDirectory()).getDirectoryHandle(DIRECTORY, { create: true });
}
export async function writeDurable(data: ArrayBuffer): Promise<DurableReference> {
  if (!data.byteLength || data.byteLength > MAX_BYTES)
    throw Error('Durable camera storage requires 1..256 MiB.');
  if (!navigator.storage?.getDirectory) return { backend: 'indexeddb', data };
  let root: FileSystemDirectoryHandle;
  try {
    root = await directory();
  } catch (error) {
    // WebKit ephemeral contexts can deny the OPFS root with UnknownError.
    // No candidate exists yet; publish durable bytes through IndexedDB instead.
    if (unsupported(error) || (error instanceof DOMException && error.name === 'UnknownError'))
      return { backend: 'indexeddb', data };
    throw error;
  }
  const file = crypto.randomUUID() + '.bin';
  let stage = 'create file';
  try {
    const handle: FileSystemFileHandle = await root.getFileHandle(file, { create: true });
    if (typeof handle.createWritable === 'function') {
      stage = 'open writer';
      const writer = await handle.createWritable();
      try {
        stage = 'write';
        await writer.write(data);
        stage = 'close';
        await writer.close();
      } catch (error) {
        await writer.abort().catch(() => {});
        throw error;
      }
    } else if (handle.createSyncAccessHandle) {
      stage = 'open sync writer';
      const writer = await handle.createSyncAccessHandle();
      try {
        stage = 'sync write';
        const bytes = new Uint8Array(data);
        let at = 0;
        while (at < bytes.length) {
          const count = writer.write(bytes.subarray(at), { at });
          if (count <= 0) throw Error('Durable storage write made no progress.');
          at += count;
        }
        writer.truncate(bytes.length);
        writer.flush();
      } finally {
        writer.close();
      }
    } else throw new DOMException('No OPFS writer is available.', 'NotSupportedError');
    return { backend: 'opfs', file };
  } catch (error) {
    await root.removeEntry(file).catch(() => {});
    if (unsupported(error)) return { backend: 'indexeddb', data };
    if (error instanceof DOMException)
      throw new DOMException(`OPFS ${stage}: ${error.message}`, error.name);
    throw error;
  }
}
function filename(file: string) {
  if (!/^[0-9a-f-]{36}\.bin$/.test(file)) throw Error('Invalid durable camera file.');
  return file;
}
export async function readDurable(reference: DurableReference): Promise<ArrayBuffer> {
  if (reference.backend === 'indexeddb') {
    if (!(reference.data instanceof ArrayBuffer)) throw Error('Invalid IndexedDB camera bytes.');
    return reference.data;
  }
  if (reference.backend !== 'opfs') throw Error('Unknown durable camera storage backend.');
  const file = await (await (await directory()).getFileHandle(filename(reference.file))).getFile();
  if (!file.size || file.size > MAX_BYTES) throw Error('Durable camera file exceeds its bound.');
  return file.arrayBuffer();
}
export async function removeDurable(reference: DurableReference) {
  if (reference.backend === 'opfs') await (await directory()).removeEntry(filename(reference.file));
}
