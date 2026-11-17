/** Download events the website reports after it builds a FujiMod archive. */
export const downloadKinds = ['installable', 'sd-boot-v1', 'sd-card'] as const;
export type DownloadKind = (typeof downloadKinds)[number];

export interface DownloadEvent {
  release: string;
  kind: DownloadKind;
  firmware?: string;
  cameras: number;
  session?: string;
}

const releaseTag = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$/;
const sha256 = /^[0-9a-f]{64}$/;
const sessionId = /^[0-9a-f]{8}-[0-9a-f]{4}-[4-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

/** Accept only the fields the download counter uses and cap every value. */
export function parseDownloadEvent(value: unknown): DownloadEvent | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const { release, kind, firmware, cameras, session } = value as Record<string, unknown>;
  if (typeof release !== 'string' || !releaseTag.test(release)) return undefined;
  if (typeof kind !== 'string' || !downloadKinds.includes(kind as DownloadKind)) return undefined;
  if (cameras === undefined) return undefined;
  if (typeof cameras !== 'number' || !Number.isInteger(cameras) || cameras < 1 || cameras > 256)
    return undefined;
  if (kind === 'sd-card') {
    if (firmware !== undefined) return undefined;
  } else if (typeof firmware !== 'string' || !sha256.test(firmware)) {
    return undefined;
  }
  if (session !== undefined && (typeof session !== 'string' || !sessionId.test(session)))
    return undefined;
  return {
    release,
    kind: kind as DownloadKind,
    ...(typeof firmware === 'string' ? { firmware } : {}),
    cameras,
    ...(typeof session === 'string' ? { session } : {}),
  };
}
