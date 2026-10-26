/** Bring-up progress per camera model, from the catalog's generated progress documents. */
export type StageProgress = 'pass' | 'fail' | 'stale' | 'not-run';
export type ModProgress = StageProgress | 'unavailable';
export type StockResult = 'pass' | 'partial' | 'fail' | 'stale' | 'not-run';
export const STAGES = [
  'research',
  'analysis',
  'rec',
  'menu',
  'camera-identity',
  'camera-snapshot',
  'camera-menu',
  'mod-target',
  'mod-build',
  'sd-image',
  'sd-boot',
] as const;
export type Stage = (typeof STAGES)[number];
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
  stages: Record<Stage, StageProgress>;
  /** Stock features the body has, by feature. */
  stock: Record<string, StockProgress>;
  mods: Record<string, ModProgress>;
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
  label: string,
  weight: number,
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
  const { stages, facts } = progress,
    research = pass(stages.research),
    mods = Object.entries(progress.mods),
    working = mods.filter(([, result]) => result === 'pass').length;
  const emulator = EMULATOR.flatMap(([feature, label]): Step[] => {
    if (feature === 'boot') return [{ label, fraction: pass(stages.rec) }];
    if (feature === 'menus') return [{ label, fraction: pass(stages.menu) }];
    const stock = progress.stock[feature];
    return stock ? [stockStep(label, stock)] : [];
  });
  return [
    phase('research', 'Research', 10, [
      { label: 'Update file registered', fraction: research },
      { label: 'Overlays unpacked', fraction: research },
      { label: 'Corpus exported', fraction: research },
      { label: 'Function index built', fraction: pass(stages.analysis) },
      facts
        ? {
            label: 'Facts located',
            fraction: facts.located / facts.total,
            detail: `${facts.located} of ${facts.total}`,
          }
        : { label: 'Facts located', fraction: 0 },
      progress.boot_rom
        ? { label: 'Boot ROM', fraction: 1, detail: `${progress.boot_rom} dump` }
        : { label: 'Boot ROM', fraction: 0 },
    ]),
    phase('emulator', 'Emulator', 40, emulator, true),
    phase('camera', 'Camera', 15, [
      { label: 'ff80 identity', fraction: pass(stages['camera-identity']) },
      { label: 'RAM snapshot', fraction: pass(stages['camera-snapshot']) },
      { label: 'Remote menu control', fraction: pass(stages['camera-menu']) },
    ]),
    {
      ...phase('mods', 'Mods', 25, [
        { label: 'Mod target', fraction: pass(stages['mod-target']) },
        { label: 'FujiMod build', fraction: pass(stages['mod-build']) },
        ...mods.map(([mod, result]) => ({ label: mod, fraction: pass(result) })),
      ]),
      fraction: mods.length ? working / mods.length : 0,
      value: working ? `${working} of ${mods.length}` : 'Not yet',
    },
    phase('sd-card', 'SD-boot image', 10, [
      { label: 'Image built', fraction: pass(stages['sd-image']) },
      { label: 'Booted on a camera', fraction: pass(stages['sd-boot']) },
    ]),
  ];
}

/** Whether any stage, stock feature or mod passes; otherwise the model is listed as not started. */
export function started(progress: FirmwareProgress): boolean {
  return (
    STAGES.some((stage) => progress.stages[stage] === 'pass') ||
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
/** One document per model: the newest firmware version with recorded progress. */
export function byModel(documents: FirmwareProgress[]): Record<string, FirmwareProgress> {
  const result: Record<string, FirmwareProgress> = {};
  for (const document of documents) {
    const known = result[document.model];
    if (!known || newer(document.version, known.version)) result[document.model] = document;
  }
  return result;
}
