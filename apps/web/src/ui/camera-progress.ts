/** Bring-up progress per camera model, from the catalog's generated progress documents. */
export type StageProgress = 'pass' | 'partial' | 'fail' | 'inapplicable' | 'stale' | 'not-run';
export type ModProgress = Exclude<StageProgress, 'partial'> | 'unavailable';
export type StockResult = 'pass' | 'partial' | 'fail' | 'stale' | 'not-run';
/** Bring-up tracks, in the bring-up graph's order. */
export type TrackId = 'static' | 'emulator' | 'camera' | 'fujimod' | 'sd';
export interface StageEntry {
  id: string;
  /** The stage's title as the bring-up runner prints it. */
  title: string;
  result: StageProgress;
}
export interface TrackProgress {
  id: TrackId;
  title: string;
  /** CSS phase identifier; the graph chooses it so the site keeps no alias table. */
  phase: Phase['id'];
  stages: StageEntry[];
  /** The ordered steps the website draws; an `inapplicable` stage has none. */
  steps: StepProgress[];
}

export interface StepProgress {
  id: string;
  label: string;
  stage: string;
  result: Exclude<StageProgress, 'inapplicable'> | 'unavailable';
  passed: number;
  total: number;
  detail?: string;
  milestone?: boolean;
}
export interface StockProgress {
  result: StockResult;
  passed?: number;
  total?: number;
}
export interface FirmwareProgress {
  firmware_id: string;
  model: string;
  version: string;
  facts: { located: number; total: number } | null;
  /** Camera model whose boot ROM dump this firmware boots with. */
  boot_rom: string | null;
  /** The bring-up graph's tracks and stages with each stage's latest result. */
  tracks: TrackProgress[];
  /** Stock features the body has, by feature. */
  stock: Record<string, StockProgress>;
  mods: Record<string, ModProgress>;
}

export interface Phase {
  id: 'research' | 'emulator' | 'camera' | 'mods' | 'sd-card';
  label: string;
  passed: number;
  steps: StepProgress[];
}

export function stepPassed(step: StepProgress): boolean {
  return (
    step.result === 'pass' ||
    (step.result === 'stale' && step.total > 0 && step.passed === step.total)
  );
}

export function phases(progress: FirmwareProgress): Phase[] {
  return progress.tracks.flatMap((track) => {
    const steps = track.steps.filter((step) => step.milestone);
    return steps.length
      ? [
          {
            id: track.phase,
            label: track.title,
            passed: steps.filter(stepPassed).length,
            steps,
          },
        ]
      : [];
  });
}

/** A body with any verified engineering work is shown even before a milestone passes. */
export function started(progress: FirmwareProgress): boolean {
  return progress.tracks.some((track) =>
    track.steps.some((step) => stepPassed(step) || step.result === 'partial'),
  );
}

function versionKey(version: string) {
  return version.split('.').map(Number);
}
function newer(a: string, b: string) {
  const [x, y] = [versionKey(a), versionKey(b)];
  for (let index = 0; index < Math.max(x.length, y.length); index++)
    if ((x[index] ?? 0) !== (y[index] ?? 0)) return (x[index] ?? 0) > (y[index] ?? 0);
  return false;
}
/** One document per model: the newest firmware version with recorded progress. */
export function byModel(documents: FirmwareProgress[]): Record<string, FirmwareProgress> {
  const result: Record<string, FirmwareProgress> = {};
  for (const document of documents) {
    const known = result[document.model];
    if (!known || newer(document.version, known.version)) result[document.model] = document;
  }
  return result;
}
