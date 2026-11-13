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
  mods: number;
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
      const id = sdBootV2Firmware(entry.id);
      return processor &&
        sdBootV2For(processor) &&
        !sdBootV2Files(card, processor, id).missing.length
        ? [{ id, model: entry.model, version: entry.version, processor, mods: entry.mods.length }]
        : [];
    })
    .sort((a, b) => b.mods - a.mods || a.model.localeCompare(b.model));
}

/** Root files for one camera: its own files, those shared by its processor, and what is missing.
 *
 * FFBOOT.BIN carries the body's early LED rows, so the release publishes one
 * per body; FFSYS.BIN and the stage-2 image are shared by the whole processor.
 * A file tagged with `firmware` is written only for that camera. */
export function sdBootV2Files(card: readonly SdCardFile[], processor: string, firmware: string) {
  const files = card.filter(
    (file) =>
      file.processor === processor && (file.firmware === undefined || file.firmware === firmware),
  );
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
    for (const file of sdBootV2Files(card, camera.processor, camera.id).files)
      required.set(file.path, file.sha256);
  return new Set(
    cameras
      .filter((camera) =>
        sdBootV2Files(card, camera.processor, camera.id).files.some(
          (file) =>
            required.has(file.path) && required.get(file.path) !== file.sha256,
        ),
      )
      .map((camera) => camera.id),
  );
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
  const boot = new Map<string, SdCardFile>();
  for (const camera of input.cameras) {
    const needed = sdBootV2Files(input.card, camera.processor, camera.id);
    if (needed.missing.length)
      problems.push(
        `This release has no ${needed.missing.join(', ')} for the ${camera.model}.`,
      );
    for (const file of needed.files) {
      const previous = boot.get(file.path);
      if (previous && previous.sha256 !== file.sha256)
        problems.push(
          `The ${camera.model} needs a different ${file.path} than another chosen camera. Choose one of them.`,
        );
      else if (!previous) {
        boot.set(file.path, file);
        files.push({ ...file, from: 'release' });
      }
    }
  }
  for (const file of input.card)
    if (file.processor === undefined && !input.excluded.has(file.path))
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
