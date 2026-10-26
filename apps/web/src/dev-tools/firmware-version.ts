import type { FirmwareVersion } from 'fujimod';
import type { CameraIdentity } from './camera-model';

function version(value: string): number | undefined {
  const match = /^(\d{1,2})\.(\d{2})$/.exec(value.trim());
  return match ? Number(match[1]) * 100 + Number(match[2]) : undefined;
}

const productName = (value: string) =>
  value
    .trim()
    .replace(/^FUJIFILM\s+/i, '')
    .toUpperCase();

/** Versions are ordered within one camera model, never across models. */
export function firmwareVersion(
  identity: CameraIdentity | undefined,
  releases: readonly FirmwareVersion[],
) {
  if (!identity?.product || !identity.internalVersion) return;
  const current = version(identity.internalVersion);
  if (current === undefined) return;
  const product = productName(identity.product);
  const known = releases.filter((release) => productName(release.product) === product);
  let latest: FirmwareVersion | undefined;
  let highest = -1;
  const matches: string[] = [];
  for (const release of known) {
    const internal = version(release.internal_version);
    if (internal === undefined) continue;
    if (internal === current && !matches.includes(release.version)) matches.push(release.version);
    if (
      internal > highest ||
      (internal === highest &&
        (version(release.version) ?? -1) > (version(latest?.version ?? '') ?? -1))
    ) {
      highest = internal;
      latest = release;
    }
  }
  if (!latest) return;
  return { publicVersion: matches.join(' / ') || undefined, latest, outdated: current < highest };
}
