/** Streaming deflate ZIP construction; only compressed bytes are collected. */
const table = Uint32Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (c & 1 ? 0xedb88320 : 0);
  return c >>> 0;
});
function updateCRC(crc: number, data: Uint8Array) {
  for (const value of data) crc = (crc >>> 8) ^ table[(crc ^ value) & 255];
  return crc;
}
export function crc32(data: Uint8Array) {
  return (updateCRC(0xffffffff, data) ^ 0xffffffff) >>> 0;
}
const max32 = 0xffffffff;
function extra(values: number[]) {
  if (!values.length) return new Uint8Array(0);
  const bytes = new Uint8Array(4 + values.length * 8),
    view = new DataView(bytes.buffer);
  view.setUint16(0, 1, true);
  view.setUint16(2, values.length * 8, true);
  values.forEach((value, i) => view.setBigUint64(4 + i * 8, BigInt(value), true));
  return bytes;
}
export async function zipBlob(entries: { path: string; data: Blob }[]): Promise<Blob> {
  const local: (Blob | Uint8Array<ArrayBuffer>)[] = [],
    directory: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0,
    directorySize = 0,
    needs64 = false;
  for (const { path, data } of entries) {
    const filename = new TextEncoder().encode(path);
    if (filename.length > 65535) throw Error('ZIP path exceeds 65535 UTF-8 bytes.');
    let crc = 0xffffffff,
      bytes = 0;
    const compressed =
      data.size === 0
        ? new Blob([new Uint8Array([3, 0])])
        : await new Response(
            data
              .stream()
              .pipeThrough(
                new TransformStream<Uint8Array<ArrayBuffer>, Uint8Array<ArrayBuffer>>({
                  transform(chunk, controller) {
                    crc = updateCRC(crc, chunk);
                    bytes += chunk.byteLength;
                    controller.enqueue(chunk);
                  },
                }),
              )
              .pipeThrough(new CompressionStream('deflate-raw')),
          ).blob();
    crc = (crc ^ 0xffffffff) >>> 0;
    const size64 = bytes >= max32 || compressed.size >= max32,
      offset64 = offset >= max32;
    needs64 ||= size64 || offset64;
    const localExtra = extra(size64 ? [bytes, compressed.size] : []);
    const header = new Uint8Array(30 + filename.length + localExtra.length),
      view = new DataView(header.buffer);
    view.setUint32(0, 0x04034b50, true);
    view.setUint16(4, size64 ? 45 : 20, true);
    view.setUint16(6, 0x800, true); // UTF-8 paths
    view.setUint16(8, 8, true); // raw deflate
    view.setUint16(12, 33, true); // fixed 1980-01-01 DOS date
    view.setUint32(14, crc, true);
    view.setUint32(18, size64 ? max32 : compressed.size, true);
    view.setUint32(22, size64 ? max32 : bytes, true);
    view.setUint16(26, filename.length, true);
    view.setUint16(28, localExtra.length, true);
    header.set(filename, 30);
    header.set(localExtra, 30 + filename.length);
    local.push(header, compressed);
    const centralExtra = extra([
      ...(size64 ? [bytes, compressed.size] : []),
      ...(offset64 ? [offset] : []),
    ]);
    const central = new Uint8Array(46 + filename.length + centralExtra.length),
      cv = new DataView(central.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 45, true);
    cv.setUint16(6, size64 || offset64 ? 45 : 20, true);
    cv.setUint16(8, 0x800, true);
    cv.setUint16(10, 8, true);
    cv.setUint16(14, 33, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, size64 ? max32 : compressed.size, true);
    cv.setUint32(24, size64 ? max32 : bytes, true);
    cv.setUint16(28, filename.length, true);
    cv.setUint16(30, centralExtra.length, true);
    cv.setUint32(42, offset64 ? max32 : offset, true);
    central.set(filename, 46);
    central.set(centralExtra, 46 + filename.length);
    directory.push(central);
    directorySize += central.length;
    offset += header.length + compressed.size;
    if (!Number.isSafeInteger(offset)) throw Error('ZIP exceeds the supported size.');
  }
  needs64 ||= entries.length >= 65535 || directorySize >= max32 || offset >= max32;
  const ending: Uint8Array<ArrayBuffer>[] = [];
  if (needs64) {
    const end64 = new Uint8Array(56),
      view = new DataView(end64.buffer);
    view.setUint32(0, 0x06064b50, true);
    view.setBigUint64(4, 44n, true);
    view.setUint16(12, 45, true);
    view.setUint16(14, 45, true);
    view.setBigUint64(24, BigInt(entries.length), true);
    view.setBigUint64(32, BigInt(entries.length), true);
    view.setBigUint64(40, BigInt(directorySize), true);
    view.setBigUint64(48, BigInt(offset), true);
    const locator = new Uint8Array(20),
      lv = new DataView(locator.buffer);
    lv.setUint32(0, 0x07064b50, true);
    lv.setBigUint64(8, BigInt(offset + directorySize), true);
    lv.setUint32(16, 1, true);
    ending.push(end64, locator);
  }
  const end = new Uint8Array(22),
    ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, Math.min(entries.length, 65535), true);
  ev.setUint16(10, Math.min(entries.length, 65535), true);
  ev.setUint32(12, Math.min(directorySize, max32), true);
  ev.setUint32(16, Math.min(offset, max32), true);
  return new Blob([...local, ...directory, ...ending, end], { type: 'application/zip' });
}
