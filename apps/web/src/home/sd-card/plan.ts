import type { CardFile, PatchFirmware } from '../../patch/catalog';
import type { Firmware } from '../../catalog';
import {
  firstInstallManifest,
  firstInstallState,
  otaFirmware,
  storeName,
  type OtaRelease,
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
export interface ProcessorBoot {
  name: string;
  stage2: string;
}
const processorBoot: Record<string, ProcessorBoot> = {
  'EXR Processor II': { name: 'EXR II', stage2: 'FM_EXR2.BIN' },
};
export function bootFor(processor: string): ProcessorBoot | undefined {
  return processorBoot[processor];
}

/** Every release camera with a known processor, most mods first. */
export function releaseCameras(
  firmwares: readonly PatchFirmware[],
  catalog: readonly Firmware[],
): CardCamera[] {
  return firmwares
    .flatMap((entry) => {
      const processor = catalog.find((known) => known.sha256 === entry.sha256)?.processor;
      return processor
        ? [
            {
              id: otaFirmware(entry.id),
              model: entry.model,
              version: entry.version,
              processor,
              mods: entry.mods.length,
            },
          ]
        : [];
    })
    .sort((a, b) => b.mods - a.mods || a.model.localeCompare(b.model));
}

/** Root files written for one processor: the release's files, and the documented names it lacks. */
export function bootFiles(card: readonly CardFile[], processor: string) {
  const files = card.filter((file) => file.processor === processor);
  const boot = bootFor(processor);
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
  card: readonly CardFile[],
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

/** One file of the download and where its bytes come from. */
export type PlannedFile = { path: string } & (
  | { from: 'release'; asset: string; sha256: string; bytes: number }
  | { from: 'store'; name: string; sha256: string; bytes: number }
  | { from: 'data'; data: Uint8Array }
);

export interface CardPlanInput {
  cameras: readonly CardCamera[];
  card: readonly CardFile[];
  releases: ReadonlyMap<string, OtaRelease>;
  /** Optional card files the user unchecked. */
  excluded: ReadonlySet<string>;
}

const json = (value: unknown) => new TextEncoder().encode(`${JSON.stringify(value, null, 2)}\n`);

/** The download's files, or why it cannot be made. */
export function cardPlan(input: CardPlanInput): { files: PlannedFile[]; problems: string[] } {
  const files: PlannedFile[] = [],
    problems: string[] = [];
  if (!input.cameras.length) return { files, problems: ['Choose a camera.'] };
  for (const processor of new Set(input.cameras.map((camera) => camera.processor))) {
    const boot = bootFiles(input.card, processor);
    if (boot.missing.length)
      problems.push(`This release has no ${boot.missing.join(', ')} for ${processor} cameras.`);
    for (const file of boot.files) files.push({ ...file, from: 'release' });
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
      { path: `${system}/A.JSN`, from: 'data', data: json(firstInstallManifest(release)) },
      { path: `${system}/STATE.JSN`, from: 'data', data: json(firstInstallState()) },
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
