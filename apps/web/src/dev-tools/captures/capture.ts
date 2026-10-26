/** RAM captures through a firmware's catalog bulk path. */
import { catalog, type CaptureScope, type ResolvedSelection } from 'fujimod';
import type { CameraIdentity, ToolSession } from '../camera-model';
import type { ControlDiff } from './mapping';

/** Bytes requested per qualified bulk read. */
export const PIECE_BYTES = 16 << 20;
export const MIB = 1 << 20;

/** The catalog firmware whose located bulk path matches the camera's reported identity. */
export function bulkSelection(identity: CameraIdentity | undefined): ResolvedSelection | undefined {
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

/** Read every range in pieces, reporting bytes done of the total. */
export async function captureRanges(
  session: ToolSession,
  firmwareId: string,
  scope: CaptureScope,
  progress: (done: number, total: number) => void,
  pieceBytes = PIECE_BYTES,
): Promise<Pick<StateCapture, 'ranges' | 'overwritten'>> {
  const total = scopeBytes(scope);
  let done = 0;
  const overwritten = new Map<string, { address: number; size: number }>();
  const ranges: RangeCapture[] = [];
  progress(0, total);
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
      const size = Math.min(pieceBytes, range.size - offset);
      const read = await session.readBulk(firmwareId, range.address + offset, size);
      if (read.data.length !== size) throw Error('Bulk read returned a partial piece');
      const data = new Uint8Array(read.data);
      capture.parts.push(data);
      capture.pieces.push({ offset, size, sha256: await sha256(data) });
      if (read.overwritten)
        overwritten.set(`${read.overwritten.address}:${read.overwritten.size}`, read.overwritten);
      done += size;
      progress(done, total);
    }
  }
  return { ranges, overwritten: [...overwritten.values()] };
}

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
