/** RAM captures through a firmware's catalog bulk path. */
import {
  catalog,
  type CaptureScope,
  type FirmwareBulk,
  type FirmwareVersion,
  type ResolvedSelection,
} from 'fujimod';
import type { CameraIdentity, ToolSession } from '../camera-model';
import type { ControlDiff } from './mapping';
import { generationFailure } from '../generation';

/** Bytes requested per qualified bulk read. */
export const PIECE_BYTES = 16 << 20;
export const MIB = 1 << 20;

/** The execution profile whose bulk path matches the camera's reported identity. */
export function executionSelection(
  identity: CameraIdentity | undefined,
): ResolvedSelection | undefined {
  const product = identity?.product?.trim();
  const version = identity?.internalVersion?.trim();
  if (!product || !version) return undefined;
  return catalog
    .selections()
    .find(
      (selection) =>
        selection.bulk?.product === product && selection.bulk.internal_version === version,
    );
}

/** Bulk facts and release identity, with physical controls when an execution profile has them. */
export interface BulkSelection {
  bulk: FirmwareBulk;
  firmware: FirmwareVersion;
  physical_controls: ResolvedSelection['physical_controls'];
}

export function bulkSelection(identity: CameraIdentity | undefined): BulkSelection | undefined {
  const product = identity?.product?.trim();
  const version = identity?.internalVersion?.trim();
  if (!product || !version) return undefined;
  const bulk = catalog
    .bulkProfiles()
    .find((item) => item.product === product && item.internal_version === version);
  if (!bulk) return undefined;
  const firmware = catalog.firmwareVersions().find((item) => item.firmware_id === bulk.firmware_id);
  if (!firmware) return undefined;
  const physical_controls = catalog
    .selections()
    .find((item) => item.firmware.id === bulk.firmware_id)?.physical_controls;
  return { bulk, firmware, physical_controls };
}

/** Preserve the published generation failure for a camera without bulk support. */
export function bulkSupportError(identity: CameraIdentity | undefined): string {
  const product = identity?.product?.trim();
  const version = identity?.internalVersion?.trim();
  const reports = catalog.generationResults();
  const report = reports.find(
    (entry) => entry.product === product && entry.internal_version === version,
  );
  if (report) return generationFailure(report.firmware_id, 'bulk');
  const identityErrors = reports
    .filter((entry) => entry.product === product && !entry.internal_version)
    .flatMap((entry) => {
      const errors =
        entry.tables.versions?.errors ??
        entry.tables.facts?.errors.filter(
          (error) => error.locator === 'internal_version' || error.locator === 'inputs',
        ) ??
        [];
      return errors.map(
        (error) => `${entry.product} ${entry.version}: ${error.locator}: ${error.reason}`,
      );
    });
  return identityErrors.length
    ? `Firmware identity generation failed. ${identityErrors.join('; ')}`
    : 'No bulk generation result was published for this camera and firmware.';
}

export function scopeBytes(scope: CaptureScope) {
  return scope.ranges.reduce((total, range) => total + range.size, 0);
}

export interface Piece {
  offset: number;
  size: number;
  sha256: string;
}
export interface RangeCapture {
  name: string;
  address: number;
  size: number;
  parts: Uint8Array<ArrayBuffer>[];
  pieces: Piece[];
}
export interface StateCapture {
  state: string;
  label: string;
  scope: string;
  time: string;
  ranges: RangeCapture[];
  overwritten: { address: number; size: number }[];
  photo?: File;
}
export interface ControlMapping {
  /** Catalog control ID, when one was chosen. */
  control?: string;
  name: string;
  note: string;
  time: string;
  scope: string;
  address: number;
  size: number;
  diff: ControlDiff;
}
export interface CaptureSet {
  firmwareId: string;
  states: StateCapture[];
  mappings: ControlMapping[];
}

async function sha256(data: Uint8Array<ArrayBuffer>) {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', data));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** Read every range in pieces, reporting bytes done of the total. Aborting stops between pieces. */
export async function captureRanges(
  session: ToolSession,
  firmwareId: string,
  scope: CaptureScope,
  progress: (done: number, total: number) => void,
  signal?: AbortSignal,
  pieceBytes = PIECE_BYTES,
): Promise<Pick<StateCapture, 'ranges' | 'overwritten'>> {
  const total = scopeBytes(scope);
  let done = 0;
  const ranges: RangeCapture[] = [];
  progress(0, total);
  const memory = await session.memory(firmwareId);
  try {
    for (const range of scope.ranges) {
      const capture: RangeCapture = {
        name: range.name,
        address: range.address,
        size: range.size,
        parts: [],
        pieces: [],
      };
      ranges.push(capture);
      for (let offset = 0; offset < range.size; offset += pieceBytes) {
        signal?.throwIfAborted();
        const size = Math.min(pieceBytes, range.size - offset);
        const data = new Uint8Array(await memory.read(range.address + offset, size));
        signal?.throwIfAborted();
        if (data.length !== size) throw Error('Bulk read returned a partial piece');
        capture.parts.push(data);
        capture.pieces.push({ offset, size, sha256: await sha256(data) });
        done += size;
        progress(done, total);
      }
    }
    const overwritten = memory.overwritten;
    return { ranges, overwritten: overwritten ? [overwritten] : [] };
  } finally {
    await memory.close();
  }
}

export const canceled = (error: unknown) => error instanceof Error && error.name === 'AbortError';

/** One contiguous copy of a single-range capture, for comparisons. */
export function contiguous(capture: Pick<StateCapture, 'ranges'>): Uint8Array<ArrayBuffer> {
  if (capture.ranges.length !== 1) throw Error('Expected one captured range');
  const [range] = capture.ranges;
  const bytes = new Uint8Array(range.size);
  let at = 0;
  for (const part of range.parts) {
    bytes.set(part, at);
    at += part.length;
  }
  return bytes;
}

/** The scope a state capture reads: everything the catalog locates. */
export function stateScope(scopes: readonly CaptureScope[]): CaptureScope | undefined {
  return scopes.reduce<CaptureScope | undefined>(
    (largest, scope) => (!largest || scopeBytes(scope) > scopeBytes(largest) ? scope : largest),
    undefined,
  );
}
