export interface ContributionFile {
  model: string;
  serial: string;
  name: string;
  blob: Blob;
}
export interface ContributionProgress {
  done: number;
  total: number;
}
interface FileStatus {
  complete: boolean;
  parts: { partNumber: number; bytes: number }[];
}
interface Status {
  id: string;
  files: FileStatus[];
  finished: boolean;
}
const failure = 'Upload failed. Your files are still on this device. Try again.';
const partBytes = 8 * 1024 * 1024;

/** The caller retains the ID and the same files until the contribution succeeds. */
export async function contribute(
  input: { id: string; files: ContributionFile[]; manifest: unknown; message: string },
  progress: (p: ContributionProgress) => void,
  signal?: AbortSignal,
): Promise<{ id: string }> {
  const base = `https://share.fujimod.com/contributions/${encodeURIComponent(input.id)}`;
  let done = 0;
  const total = input.files.reduce((sum, file) => sum + file.blob.size, 0);
  const update = () => progress({ done, total });
  async function request(path: string, method: string, body?: Blob | string): Promise<unknown> {
    signal?.throwIfAborted();
    const response = await fetch(base + path, {
      method,
      body,
      signal,
      credentials: 'omit',
      headers: typeof body === 'string' ? { 'Content-Type': 'application/json' } : undefined,
    });
    if (!response.ok) throw Error(failure);
    return response.json();
  }
  try {
    update();
    // Creation returns the same status as GET, avoiding a separate round trip on every retry.
    const status = (await request(
      '',
      'POST',
      JSON.stringify({
        files: input.files.map(({ model, serial, name, blob }) => ({
          model,
          serial,
          name,
          bytes: blob.size,
        })),
      }),
    )) as Status;
    if (
      status.id !== input.id.toLowerCase() ||
      !Array.isArray(status.files) ||
      status.files.length !== input.files.length
    )
      throw Error(failure);
    if (!status.finished) {
      for (const [index, file] of input.files.entries()) {
        const current = status.files[index];
        if (current.complete) {
          done += file.blob.size;
          update();
          continue;
        }
        const path = `/files/${index}`;
        const started = (await request(path, 'POST')) as FileStatus;
        if (started.complete) {
          done += file.blob.size;
          update();
          continue;
        }
        const saved = new Map(started.parts.map((part) => [part.partNumber, part.bytes]));
        const source = file.blob
          .stream()
          .pipeThrough(
            new TransformStream<Uint8Array<ArrayBuffer>, Uint8Array<ArrayBuffer>>({
              transform(chunk, controller) {
                signal?.throwIfAborted();
                done += chunk.byteLength;
                update();
                controller.enqueue(chunk);
              },
            }),
          )
          .pipeThrough(new CompressionStream('gzip'));
        const reader = source.getReader();
        const abort = () => {
          void reader.cancel(signal?.reason).catch(() => {});
        };
        signal?.addEventListener('abort', abort, { once: true });
        let pending: Uint8Array<ArrayBuffer>[] = [],
          size = 0,
          partNumber = 0;
        async function upload() {
          const blob = new Blob(pending);
          pending = [];
          size = 0;
          partNumber++;
          if (saved.has(partNumber)) {
            if (saved.get(partNumber) !== blob.size) throw Error(failure);
            return;
          }
          for (let attempt = 0; ; attempt++) {
            try {
              await request(`${path}/parts/${partNumber}`, 'PUT', blob);
              return;
            } catch (error) {
              signal?.throwIfAborted();
              if (attempt >= 3) throw error;
            }
          }
        }
        try {
          for (;;) {
            signal?.throwIfAborted();
            const chunk = await reader.read();
            if (chunk.done) break;
            let offset = 0;
            while (offset < chunk.value.byteLength) {
              const length = Math.min(partBytes - size, chunk.value.byteLength - offset);
              pending.push(chunk.value.subarray(offset, offset + length));
              offset += length;
              size += length;
              if (size === partBytes) await upload();
            }
          }
          if (size) await upload();
          signal?.throwIfAborted();
          await request(`${path}/complete`, 'POST', JSON.stringify({ parts: partNumber }));
        } finally {
          signal?.removeEventListener('abort', abort);
          await reader.cancel().catch(() => {});
          reader.releaseLock();
        }
      }
      const receipt = (await request(
        '/finish',
        'POST',
        JSON.stringify({ manifest: input.manifest, message: input.message }),
      )) as { id: string };
      if (receipt.id !== status.id) throw Error(failure);
    }
    done = total;
    update();
    return { id: status.id };
  } catch (error) {
    signal?.throwIfAborted();
    throw Error(failure, { cause: error });
  }
}
