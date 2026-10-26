import { inspectPatch, sha256 } from './patcher.ts';
export interface PatchFirmware {
  id: string;
  model: string;
  version: string;
  sha256: string;
  bytes: number;
  release_date: string;
  download_url: string;
  patch_sha256: string;
  patch_bytes: number;
  boot_instructions: string[];
  mods?: string[];
}
export interface PatchIndex {
  schema: number;
  release: string;
  built_utc?: string;
  patches: Record<string, string>;
  firmwares: PatchFirmware[];
}
const hash = /^[0-9a-f]{64}$/;
const calendarDate = (value: unknown) => {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(value + 'T00:00:00Z');
  return !Number.isNaN(date.valueOf()) && date.toISOString().slice(0, 10) === value;
};
export function validateIndex(value: unknown): PatchIndex {
  const index = value as PatchIndex;
  if (
    index?.schema !== 1 ||
    typeof index.release !== 'string' ||
    !index.release ||
    (index.built_utc !== undefined &&
      (typeof index.built_utc !== 'string' || Number.isNaN(Date.parse(index.built_utc)))) ||
    !index.patches ||
    !Array.isArray(index.firmwares) ||
    !index.firmwares.length
  )
    throw new Error('The release index is invalid.');
  const seen = new Set();
  for (const entry of index.firmwares) {
    const filename = index.patches[entry.sha256];
    if (
      !hash.test(entry.sha256) ||
      seen.has(entry.sha256) ||
      !calendarDate(entry.release_date) ||
      !/^[a-zA-Z0-9][a-zA-Z0-9_.-]*\.fmpatch$/.test(filename) ||
      !hash.test(entry.patch_sha256) ||
      !Number.isSafeInteger(entry.bytes) ||
      entry.bytes <= 0 ||
      !Array.isArray(entry.boot_instructions) ||
      !entry.boot_instructions.length ||
      entry.boot_instructions.length > 20 ||
      !entry.boot_instructions.every(
        (step) => typeof step === 'string' && step.length && step.length < 2000,
      ) ||
      !Number.isSafeInteger(entry.patch_bytes) ||
      entry.patch_bytes <= 0 ||
      entry.patch_bytes > 64 * 1024 * 1024 ||
      ![entry.id, entry.model, entry.version].every((v) => typeof v === 'string' && v.length) ||
      new URL(entry.download_url).protocol !== 'https:'
    )
      throw new Error('A supported firmware entry is invalid.');
    seen.add(entry.sha256);
  }
  if (
    Object.keys(index.patches).length !== seen.size ||
    new Set(Object.values(index.patches)).size !== seen.size
  )
    throw new Error('The release patch map is inconsistent.');
  return index;
}

export async function verifyPatch(index: PatchIndex, entry: PatchFirmware, data: Uint8Array) {
  if (data.length !== entry.patch_bytes || (await sha256(data)) !== entry.patch_sha256)
    throw new Error('The downloaded patch failed its integrity check.');
  const { meta, program } = inspectPatch(data);
  if (
    meta.source.sha256 !== entry.sha256 ||
    meta.source.bytes !== entry.bytes ||
    meta.firmware !== entry.id ||
    meta.release !== index.release ||
    !Array.isArray(meta.mods) ||
    !meta.mods.every((name) => typeof name === 'string') ||
    (await sha256(program)) !== meta.program_sha256
  )
    throw new Error('The patch does not match its release index.');
  return meta;
}
