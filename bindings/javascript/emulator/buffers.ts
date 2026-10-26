export interface BufferFile {
  path: string;
  bytes: Uint8Array;
}

export interface BufferFileSpan {
  path: string;
  offset: number;
  length: number;
}

export interface PackedBuffers {
  rows: BufferFileSpan[];
  bytes: Uint8Array<ArrayBuffer>;
}

/** Copy a view into an independently owned ArrayBuffer. */
export function copyBuffer(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return bytes.slice();
}

/** Pack files into one owned buffer and retain their checked extents. */
export function packBuffers(files: readonly BufferFile[]): PackedBuffers {
  let length = 0;
  const rows = files.map(({ path, bytes }) => {
    if (!Number.isSafeInteger(length) || length + bytes.length > 0xffffffff)
      throw new Error('Packed buffers exceed the supported extent.');
    const row = { path, offset: length, length: bytes.length };
    length += bytes.length;
    return row;
  });
  const bytes = new Uint8Array(length);
  files.forEach((file, index) => bytes.set(file.bytes, rows[index]!.offset));
  return { rows, bytes };
}

/** Copy a bounded section of an owned byte buffer. */
export function copySpan(bytes: Uint8Array, span: { offset: number; length: number }) {
  if (
    !Number.isSafeInteger(span.offset) ||
    !Number.isSafeInteger(span.length) ||
    span.offset < 0 ||
    span.length < 0 ||
    span.offset + span.length > bytes.length
  )
    throw new Error('Invalid buffer span.');
  return bytes.slice(span.offset, span.offset + span.length);
}
