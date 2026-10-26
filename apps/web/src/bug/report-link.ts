import { crc32 } from '@fujimod/emulator-ui/archive';
export const MAX_REPORT_BYTES = 64 * 1024 * 1024;
export interface ReportLink {
  v: 1;
  firmware: 'x70-110';
  commit: string;
  path: string;
  bytes: number;
  crc32: string;
  files: string;
  archive?: string;
}
export function base64url(bytes: Uint8Array): string {
  let text = '';
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
export function decode64(text: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1)
    throw Error('Invalid report encoding.');
  return Uint8Array.from(atob(text.replaceAll('-', '+').replaceAll('_', '/')), (c) =>
    c.charCodeAt(0),
  );
}
export function parseReportLink(hash: string): ReportLink {
  const encoded = new URLSearchParams(hash.replace(/^#\??/, '')).get('report');
  if (!encoded || encoded.length > MAX_REPORT_BYTES * 2)
    throw Error('This link has no valid camera report.');
  const wire = JSON.parse(
    new TextDecoder('utf-8', { fatal: true, ignoreBOM: false }).decode(decode64(encoded)),
  );
  if (
    !wire ||
    typeof wire !== 'object' ||
    Array.isArray(wire) ||
    typeof wire.r !== 'string' ||
    !/^[a-fA-F0-9]{7}$/.test(wire.r) ||
    Object.keys(wire).some((key) => !['v', 'b', 'r', 'n', 'c', 'f', 'a'].includes(key))
  )
    throw Error('Unsupported camera report link.');
  return parseReportMetadata({
    v: wire.v,
    firmware: 'x70-110',
    commit: wire.b,
    path: `A:\\FUJIMOD\\REPORTS\\R${wire.r}.TAR`,
    bytes: wire.n,
    crc32: wire.c,
    files: wire.f,
    ...(wire.a === undefined ? {} : { archive: wire.a }),
  });
}
/** Expanded upload metadata; the full build identity stays in manifest.json. */
export function parseReportMetadata(value: ReportLink): ReportLink {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    value.v !== 1 ||
    value.firmware !== 'x70-110' ||
    typeof value.commit !== 'string' ||
    !/^[a-f0-9]{8}$/.test(value.commit) ||
    typeof value.path !== 'string' ||
    !/^[A-Z]:\\FUJIMOD\\REPORTS\\R[a-fA-F0-9]{7}\.TAR$/.test(value.path) ||
    !Number.isSafeInteger(value.bytes) ||
    value.bytes < 1024 ||
    value.bytes > MAX_REPORT_BYTES ||
    typeof value.crc32 !== 'string' ||
    !/^[a-f0-9]{8}$/.test(value.crc32) ||
    typeof value.files !== 'string' ||
    !/^[a-f0-9]{8}$/.test(value.files) ||
    (value.archive !== undefined && typeof value.archive !== 'string')
  )
    throw Error('Unsupported camera report link.');
  return value;
}
export function reportName(link: ReportLink): string {
  return link.path.split('\\').at(-1)!;
}
/** Checks the complete archive before making it available for reporting. No extraction to disk. */
export interface ReportEntry {
  name: string;
  bytes: number;
  /** Where the entry's data starts in the TAR. */
  offset: number;
}
export function reportEntries(bytes: Uint8Array<ArrayBuffer>, link: ReportLink): ReportEntry[] {
  if (bytes.length !== link.bytes || crc32(bytes).toString(16).padStart(8, '0') !== link.crc32)
    throw Error('The report is incomplete or belongs to another capture.');
  if (bytes.length % 512 || bytes.length < 1024) throw Error('Invalid TAR archive.');
  let offset = 0,
    ended = false;
  const names = new Set<string>();
  const entries: ReportEntry[] = [];
  const text = (start: number, size: number) =>
    new TextDecoder().decode(bytes.subarray(start, start + size)).split('\0')[0];
  while (offset + 512 <= bytes.length) {
    const header = bytes.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      if (offset + 1024 > bytes.length || !bytes.subarray(offset).every((byte) => byte === 0))
        throw Error('Truncated TAR end marker.');
      ended = true;
      break;
    }
    const name = text(offset, 100),
      sizeField = text(offset + 124, 12).trim(),
      checksum = text(offset + 148, 8).trim();
    if (
      !/^[0-7]+$/.test(sizeField) ||
      !/^[0-7]+$/.test(checksum) ||
      !name ||
      names.has(name) ||
      !/^[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(name) ||
      name.split('/').some((part) => part === '.' || part === '..') ||
      (header[156] !== 0 && header[156] !== 48)
    )
      throw Error('Invalid TAR header.');
    let sum = 0;
    for (let i = 0; i < 512; i++) sum += i >= 148 && i < 156 ? 32 : header[i];
    if (sum !== parseInt(checksum, 8)) throw Error('Invalid TAR checksum.');
    names.add(name);
    const size = parseInt(sizeField, 8);
    entries.push({ name, bytes: size, offset: offset + 512 });
    offset += 512 + Math.ceil(size / 512) * 512;
    if (offset > bytes.length) throw Error('Truncated TAR entry.');
  }
  if (!ended) throw Error('The report archive has no end marker.');
  return entries;
}
export function validateReport(bytes: Uint8Array<ArrayBuffer>, link: ReportLink): Blob {
  reportEntries(bytes, link);
  return new Blob([bytes], { type: 'application/x-tar' });
}
