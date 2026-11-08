/** Paths served from the current release for camera updates. */
export type OtaRoute = { kind: 'manifest'; firmware: string } | { kind: 'store'; name: string };

/** Firmware IDs accepted by the release manifest tool. */
export const otaFirmware = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}$/;
/** A store name: eight SHA-256 hex digits, a dot and three more. */
export const otaStoreName = /^[0-9A-F]{8}\.[0-9A-F]{3}$/;

export function otaRoute(path: string): OtaRoute | undefined {
  if (path.startsWith('/ota/store/')) {
    const name = path.slice('/ota/store/'.length);
    return otaStoreName.test(name) ? { kind: 'store', name } : undefined;
  }
  const match = /^\/ota\/([^/]+)\/latest\.json$/.exec(path);
  if (!match || !otaFirmware.test(match[1])) return;
  return { kind: 'manifest', firmware: match[1] };
}

export interface ByteRange {
  /** First byte, inclusive. */
  start: number;
  /** Last byte, inclusive. */
  end: number;
}

/** Parse one `bytes=` range. A header this function ignores (`undefined`) is
 * absent, malformed or names several ranges; the caller sends the whole file. */
export function byteRange(
  header: string | null,
  size: number,
): ByteRange | 'unsatisfiable' | undefined {
  const match = header ? /^bytes=(\d*)-(\d*)$/i.exec(header.trim()) : null;
  if (!match || (!match[1] && !match[2])) return;
  if (size === 0) return 'unsatisfiable';
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (suffix === 0) return 'unsatisfiable';
    return { start: Math.max(0, size - suffix), end: size - 1 };
  }
  const start = Number(match[1]);
  const end = match[2] ? Number(match[2]) : size - 1;
  if (match[2] && end < start) return;
  if (start >= size) return 'unsatisfiable';
  return { start, end: Math.min(end, size - 1) };
}
