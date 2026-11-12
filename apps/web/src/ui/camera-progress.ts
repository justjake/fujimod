/** Bring-up progress per camera model, from the catalog's generated progress documents. */
export type StageProgress = 'pass' | 'partial' | 'fail' | 'stale' | 'not-run';
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
  /** Share of the bar assigned to this track. */
  weight: number;
  /** Draw each step as its own cell instead of one continuous fill. */
  cells: boolean;
  stages: StageEntry[];
  /** The ordered steps the website draws. */
  steps: StepProgress[];
}

export interface StepProgress {
  id: string;
  label: string;
  stage: string;
  result: StageProgress | 'unavailable';
  passed: number;
  total: number;
  detail?: string;
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
  /** Hand-authored publishing override from `progress.override.json`; never recorded. */
  override?: ProgressOverride;
}
/** A maintainer's decision to publish a firmware's progress differently from its records. */
export interface ProgressOverride {
  firmware_id: string;
  /** Show every phase and step as complete. */
  complete: boolean;
}

export interface Step {
  label: string;
  /** Completed share of this step, 0–1. */
  fraction: number;
  /** "35 of 35", "9 of 11", "X70 dump". */
  detail?: string;
}
export interface Phase {
  id: 'research' | 'emulator' | 'camera' | 'mods' | 'sd-card';
  label: string;
  /** Share of the bar's width. */
  weight: number;
  /** Completed share of this phase, 0–1. */
  fraction: number;
  /** "Done", "2 of 3", "6 of 24", "Not yet". */
  value: string;
  steps: Step[];
  /** Draw each step as its own cell of the segment. */
  cells: boolean;
}

function phase(
  id: Phase['id'],
  weight: number,
  label: string,
  steps: Step[],
  cells = false,
): Phase {
  const done = steps.filter((step) => step.fraction === 1).length;
  return {
    id,
    label,
    weight,
    fraction: steps.reduce((sum, step) => sum + step.fraction, 0) / steps.length,
    value: done === steps.length ? 'Done' : done ? `${done} of ${steps.length}` : 'Not yet',
    steps,
    cells,
  };
}

export function phases(progress: FirmwareProgress): Phase[] {
  const recorded = progress.tracks.map((track) =>
    phase(
      track.phase,
      track.weight,
      track.title,
      track.steps.map((step) => ({
        label: step.label,
        fraction: step.total ? step.passed / step.total : 0,
        detail: step.detail,
      })),
      track.cells,
    ),
  );
  if (!progress.override?.complete) return recorded;
  return recorded.map((phase) => {
    const steps = phase.steps.map((step) =>
      step.fraction === 1 ? step : { label: step.label, fraction: 1 },
    );
    const mods =
      progress.tracks
        .find((track) => track.phase === 'mods')
        ?.steps.filter((step) => step.stage === 'walkthroughs').length ?? 0;
    return {
      ...phase,
      fraction: 1,
      value: phase.id === 'mods' ? `${mods} of ${mods}` : 'Done',
      steps,
    };
  });
}

/** Whether any stage, stock feature or mod passes; otherwise the model is listed as not started. */
export function started(progress: FirmwareProgress): boolean {
  return (
    !!progress.override?.complete ||
    progress.tracks.some((track) =>
      track.steps.some((step) => step.result === 'pass' || step.result === 'partial'),
    )
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
/** Attach each firmware's publishing override to its generated progress document. */
export function withOverrides(
  documents: FirmwareProgress[],
  overrides: ProgressOverride[],
): FirmwareProgress[] {
  const byFirmware = new Map(overrides.map((override) => [override.firmware_id, override]));
  for (const override of overrides)
    if (!documents.some((document) => document.firmware_id === override.firmware_id))
      throw new Error(`Progress override for unknown firmware ${override.firmware_id}`);
  return documents.map((document) => {
    const override = byFirmware.get(document.firmware_id);
    return override ? { ...document, override } : document;
  });
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
