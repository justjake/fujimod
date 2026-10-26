import { digest } from './identity';
/** Preserve register/state detail while bounding bulk buffers in downloadable reports. */
export async function diagnosticReport(value: unknown): Promise<unknown> {
  if (Array.isArray(value)) {
    if (value.length > 4096) {
      const bytes = value.every((n) => Number.isInteger(n) && n >= 0 && n <= 255);
      const data = bytes ? new Uint8Array(value) : new TextEncoder().encode(JSON.stringify(value));
      return {
        encoding: bytes ? 'bytes-sha256' : 'json-sha256',
        length: value.length,
        bytes: data.byteLength,
        sha256: await digest(data),
        preview: await diagnosticReport(value.slice(0, 32)),
      };
    }
    const result = [];
    for (const item of value) result.push(await diagnosticReport(item));
    return result;
  }
  if (value && typeof value === 'object') {
    const result: Record<string, unknown> = {};
    for (const [name, item] of Object.entries(value)) result[name] = await diagnosticReport(item);
    return result;
  }
  return value;
}
