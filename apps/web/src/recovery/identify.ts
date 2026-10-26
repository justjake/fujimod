import { catalog as reference, recovery, type PhysicalControls } from 'fujimod';
import type { DownloadRow } from '../cameras/FirmwareDownloads';
import type { IdentifiedFirmware } from '../cameras/store';

/** What a firmware's recovery card holds and how its camera boots from the card. */
export interface RecoveryPlan {
  model: string;
  canBuild: boolean;
  missing: string[];
  files: string[];
  /** How to hold the camera to boot from the SD card, when the catalog documents it. */
  sdBoot?: PhysicalControls;
}

function controlsFor(firmwareId: string): PhysicalControls | undefined {
  return (
    reference.selections().find((selection) => selection.firmware.id === firmwareId)
      ?.physical_controls ?? undefined
  );
}

/** Every camera with an official release in the catalog, newest release each, by name, GFX last. */
export function recoveryCameras(): DownloadRow[] {
  const newest = new Map<string, DownloadRow>();
  for (const release of reference.firmwareVersions()) {
    const known = newest.get(release.product);
    if (
      known?.version &&
      known.version.localeCompare(release.version, 'en', { numeric: true }) >= 0
    )
      continue;
    newest.set(release.product, {
      model: release.product,
      version: release.version,
      url: release.download_url,
    });
  }
  const gfx = (row: DownloadRow) => (row.model.startsWith('GFX') ? 1 : 0);
  return [...newest.values()].sort(
    (a, b) => gfx(a) - gfx(b) || a.model.localeCompare(b.model, 'en', { numeric: true }),
  );
}

async function sha256(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** Identify the official update `dat` by its content hash. */
export async function identify(dat: Uint8Array<ArrayBuffer>): Promise<IdentifiedFirmware> {
  if (!dat.length || dat.length > 256 * 1024 * 1024)
    throw Error('Choose a firmware DAT smaller than 256 MB.');
  const hash = await sha256(dat);
  const release = reference.firmwareVersions().find((entry) => entry.update_sha256 === hash);
  if (!release) throw Error(`Unrecognized firmware · SHA-256 ${hash}`);
  return {
    id: release.firmware_id,
    model: release.product,
    version: release.version,
    sha256: hash,
  };
}

/** The recovery card for `firmwareId`, read from its recipe without building it. */
export function recoveryPlan(firmwareId: string): RecoveryPlan {
  const model =
    reference.firmwareVersions().find((entry) => entry.firmware_id === firmwareId)?.product ??
    firmwareId;
  const status = recovery.availability(firmwareId);
  try {
    const sd = status.sdBoot;
    const controls = controlsFor(firmwareId);
    return {
      model,
      canBuild: status.canBuild,
      missing: [...status.missing],
      files: [...status.outputFilenames],
      sdBoot: sd && controls ? controls : undefined,
    };
  } finally {
    status.free();
  }
}
