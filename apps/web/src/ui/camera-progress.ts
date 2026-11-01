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
  stages: StageEntry[];
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

/** Emulator features in table order; boot and menus are the REC and menu stages. */
const EMULATOR: [string, string][] = [
  ['boot', 'Boot'],
  ['record-picture', 'Record picture'],
  ['record-movie', 'Record movie'],
  ['control-input', 'Control input'],
  ['control-layout', 'Control layout'],
  ['menus', 'Menus'],
  ['focus', 'Focus'],
  ['lens', 'Lens'],
  ['film-simulations', 'Film simulations'],
  ['playback-picture', 'Playback picture'],
  ['playback-movie', 'Playback movie'],
  ['develop-raw', 'Develop RAW'],
  ['ibis', 'IBIS'],
  ['sd', 'SD'],
  ['card-slots', 'Card slots'],
];
const pass = (result: string) => Number(result === 'pass');
function stockStep(label: string, stock: StockProgress): Step {
  if (stock.result === 'partial' && stock.passed !== undefined && stock.total)
    return {
      label,
      fraction: stock.passed / stock.total,
      detail: `${stock.passed} of ${stock.total}`,
    };
  return { label, fraction: pass(stock.result) };
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
  const recorded = recordedPhases(progress);
  if (!progress.override?.complete) return recorded;
  return recorded.map((phase) => {
    const steps = phase.steps.map((step) =>
      step.fraction === 1 ? step : { label: step.label, fraction: 1 },
    );
    const mods = Object.keys(progress.mods).length;
    return {
      ...phase,
      fraction: 1,
      value: phase.id === 'mods' ? `${mods} of ${mods}` : 'Done',
      steps,
    };
  });
}

/** The website's phase for each bring-up track. */
const PHASES: Record<TrackId, [Phase['id'], number]> = {
  static: ['research', 10],
  emulator: ['emulator', 35],
  camera: ['camera', 20],
  fujimod: ['mods', 25],
  sd: ['sd-card', 10],
};
/** Stages the emulator phase draws as its boot and menu feature cells. */
const EMULATOR_FEATURES: Record<string, string> = { rec: 'boot', menu: 'menus' };
function track(progress: FirmwareProgress, id: TrackId): StageEntry[] {
  return progress.tracks.find((entry) => entry.id === id)?.stages ?? [];
}
function stageStep(stage: StageEntry): Step {
  return { label: stage.title, fraction: pass(stage.result) };
}

function recordedPhases(progress: FirmwareProgress): Phase[] {
  const { facts } = progress,
    mods = Object.entries(progress.mods),
    working = mods.filter(([, result]) => result === 'pass').length,
    title = (id: TrackId) => progress.tracks.find((entry) => entry.id === id)?.title ?? id;
  const research = track(progress, 'static').map((stage): Step =>
    stage.id === 'analysis' && facts && stage.result !== 'pass'
      ? {
          label: stage.title,
          fraction: facts.located / facts.total,
          detail: `${facts.located} of ${facts.total} facts`,
        }
      : stageStep(stage),
  );
  research.push(
    progress.boot_rom
      ? { label: 'Boot ROM', fraction: 1, detail: `${progress.boot_rom} dump` }
      : { label: 'Boot ROM', fraction: 0 },
  );
  const emulatorStages = track(progress, 'emulator');
  const byFeature = new Map(
    emulatorStages
      .filter((stage) => EMULATOR_FEATURES[stage.id])
      .map((stage) => [EMULATOR_FEATURES[stage.id]!, stage]),
  );
  const emulator = EMULATOR.flatMap(([feature, label]): Step[] => {
    const stage = byFeature.get(feature);
    if (stage) return [{ label, fraction: pass(stage.result) }];
    const stock = progress.stock[feature];
    return stock ? [stockStep(label, stock)] : [];
  });
  emulator.push(
    ...emulatorStages
      .filter((stage) => !EMULATOR_FEATURES[stage.id] && stage.id !== 'stock')
      .map(stageStep),
  );
  const fujimod = track(progress, 'fujimod');
  const builds = fujimod.filter((stage) => stage.id !== 'walkthroughs');
  const [id, weight] = PHASES.fujimod;
  return [
    phase(...PHASES.static, title('static'), research),
    phase(...PHASES.emulator, title('emulator'), emulator, true),
    phase(...PHASES.camera, title('camera'), track(progress, 'camera').map(stageStep)),
    {
      ...phase(id, weight, title('fujimod'), [
        ...builds.map(stageStep),
        ...mods.map(([mod, result]) => ({ label: mod, fraction: pass(result) })),
      ]),
      fraction: mods.length ? working / mods.length : 0,
      value: working ? `${working} of ${mods.length}` : 'Not yet',
    },
    phase(...PHASES.sd, title('sd'), track(progress, 'sd').map(stageStep)),
  ];
}

/** Whether any stage, stock feature or mod passes; otherwise the model is listed as not started. */
export function started(progress: FirmwareProgress): boolean {
  return (
    !!progress.override?.complete ||
    progress.tracks.some((entry) => entry.stages.some((stage) => stage.result === 'pass')) ||
    Object.values(progress.stock).some(
      (stock) => stock.result !== 'not-run' && stock.result !== 'fail' && stock.result !== 'stale',
    ) ||
    Object.values(progress.mods).includes('pass')
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
