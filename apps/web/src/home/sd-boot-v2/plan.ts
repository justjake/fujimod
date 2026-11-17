import type { SdCardFile, PatchFirmware } from '../../patch/catalog';
import type { Firmware } from '../../catalog';
import {
  sdBootV2FirstInstallManifest,
  sdBootV2FirstInstallState,
  sdBootV2Firmware,
  storeName,
  type SdBootV2Release,
} from '../../ota/release';

/** A camera the release supports, as offered in the download checklist. */
export interface CardCamera {
  /** Card directory under `FUJIMOD/SYSTEM/`, such as `X70-110`. */
  id: string;
  model: string;
  version: string;
  processor: string;
  /** The mods built into this camera's release. */
  mods: readonly string[];
  /** Fujifilm's page for the camera's latest stock firmware. */
  download_url: string;
}

/** Processor generations whose SD boot files the card documents. */
export interface SdBootV2Processor {
  name: string;
  stage2: string;
}
const sdBootV2Processors: Record<string, SdBootV2Processor> = {
  'EXR Processor II': { name: 'EXR II', stage2: 'FM_EXR2.BIN' },
};
export function sdBootV2For(processor: string): SdBootV2Processor | undefined {
  return sdBootV2Processors[processor];
}

/** Every release camera whose processor has complete SD boot files, most mods first. */
export function releaseCameras(
  firmwares: readonly PatchFirmware[],
  catalog: readonly Firmware[],
  card: readonly SdCardFile[],
): CardCamera[] {
  return firmwares
    .flatMap((entry) => {
      const processor = catalog.find((known) => known.sha256 === entry.sha256)?.processor;
      return processor && sdBootV2For(processor) && !sdBootV2Files(card, processor).missing.length
        ? [
            {
              id: sdBootV2Firmware(entry.id),
              model: entry.model,
              version: entry.version,
              processor,
              mods: entry.mods,
              download_url: entry.download_url,
            },
          ]
        : [];
    })
    .sort((a, b) => b.mods.length - a.mods.length || a.model.localeCompare(b.model));
}

/** Root files written for one processor: the release's files, and the documented names it lacks. */
export function sdBootV2Files(card: readonly SdCardFile[], processor: string) {
  const files = card.filter((file) => file.processor === processor);
  const boot = sdBootV2For(processor);
  const names = boot ? ['FFBOOT.BIN', 'FFSYS.BIN', boot.stage2] : [];
  return {
    files,
    missing: names.filter((name) => !files.some((file) => file.path === name)),
  };
}

/** Cameras whose root files would replace files another chosen camera needs. */
export function conflicting(
  cameras: readonly CardCamera[],
  chosen: readonly CardCamera[],
  card: readonly SdCardFile[],
) {
  const required = new Map<string, string>();
  for (const camera of chosen)
    for (const file of card)
      if (file.processor === camera.processor) required.set(file.path, file.sha256);
  return new Set(
    cameras
      .filter((camera) =>
        card.some(
          (file) =>
            file.processor === camera.processor &&
            required.has(file.path) &&
            required.get(file.path) !== file.sha256,
        ),
      )
      .map((camera) => camera.id),
  );
}

/** Optional card folders and the mod that reads them; other folders are always offered. */
const folderMods: Readonly<Record<string, string>> = { RECIPES: 'recipes', FILMS: 'card-films' };

/** Whether an optional card file is useful to any of the cameras, by the mod that reads its folder. */
export function offered(file: SdCardFile, cameras: readonly CardCamera[]) {
  const mod = folderMods[file.path.split('/')[1] ?? ''];
  return !mod || cameras.some((camera) => camera.mods.includes(mod));
}

/** One file of the download and where its bytes come from. */
export type PlannedFile = { path: string } & (
  | { from: 'release'; asset: string; sha256: string; bytes: number }
  | { from: 'store'; name: string; sha256: string; bytes: number }
  | { from: 'data'; data: Uint8Array }
);

export interface CardPlanInput {
  cameras: readonly CardCamera[];
  card: readonly SdCardFile[];
  releases: ReadonlyMap<string, SdBootV2Release>;
  /** Optional card files the user unchecked. */
  excluded: ReadonlySet<string>;
}

const json = (value: unknown) => new TextEncoder().encode(`${JSON.stringify(value, null, 2)}\n`);

/** The download's files, or why it cannot be made. */
export function sdBootV2Plan(input: CardPlanInput): { files: PlannedFile[]; problems: string[] } {
  const files: PlannedFile[] = [],
    problems: string[] = [];
  if (!input.cameras.length) return { files, problems: ['Choose a camera.'] };
  for (const processor of new Set(input.cameras.map((camera) => camera.processor))) {
    const boot = sdBootV2Files(input.card, processor);
    if (boot.missing.length)
      problems.push(`This release has no ${boot.missing.join(', ')} for ${processor} cameras.`);
    for (const file of boot.files) files.push({ ...file, from: 'release' });
  }
  for (const file of input.card)
    if (
      file.processor === undefined &&
      !input.excluded.has(file.path) &&
      offered(file, input.cameras)
    )
      files.push({ ...file, from: 'release' });
  const store = new Map<string, PlannedFile>();
  for (const camera of input.cameras) {
    const release = input.releases.get(camera.id);
    if (!release) {
      problems.push(`The ${camera.model} release files are unavailable.`);
      continue;
    }
    const system = `FUJIMOD/SYSTEM/${camera.id}`;
    files.push(
      { path: `${system}/A.JSN`, from: 'data', data: json(sdBootV2FirstInstallManifest(release)) },
      { path: `${system}/STATE.JSN`, from: 'data', data: json(sdBootV2FirstInstallState()) },
    );
    for (const file of Object.values(release.fs)) {
      const name = storeName(file.sha256);
      store.set(name, {
        path: `FUJIMOD/SYSTEM/STORE/${name}`,
        from: 'store',
        name,
        sha256: file.sha256,
        bytes: file.size,
      });
    }
  }
  files.push(...store.values());
  return { files, problems };
}

/** Bytes the download fetches and writes, before compression. */
export function plannedBytes(files: readonly PlannedFile[]) {
  return files.reduce(
    (sum, file) => sum + (file.from === 'data' ? file.data.length : file.bytes),
    0,
  );
}
