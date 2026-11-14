import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test } from 'vite-plus/test';
import {
  byModel,
  phases,
  started,
  withOverrides,
  type FirmwareProgress,
  type TrackId,
} from './camera-progress';
import { CameraTable, type Owner } from './cameras';
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
/** A small graph-resolved step list in the generated documents' shape. */
const TRACKS: {
  id: TrackId;
  phase: 'research' | 'emulator' | 'camera' | 'mods' | 'sd-card';
  title: string;
  weight: number;
  cells?: boolean;
  steps: [string, string, string][];
}[] = [
  {
    id: 'static',
    phase: 'research',
    title: 'Static analysis',
    weight: 10,
    steps: [
      ['research', 'Update file registered', 'research'],
      ['analysis', 'Facts located', 'analysis'],
      ['boot-rom', 'Boot ROM', 'research'],
    ],
  },
  {
    id: 'emulator',
    phase: 'emulator',
    title: 'Emulator',
    weight: 35,
    cells: true,
    steps: [
      ['rec', 'Boot', 'rec'],
      ['menu', 'Menus', 'menu'],
      ['stock/film-simulations', 'Film simulations', 'stock'],
      ['stock/ibis', 'IBIS', 'stock'],
      ['stock/sd', 'SD', 'stock'],
      [
        'firmware-update-stock-installs-in-emulator',
        'Updater',
        'firmware-update-stock-installs-in-emulator',
      ],
    ],
  },
  {
    id: 'camera',
    phase: 'camera',
    title: 'Camera',
    weight: 20,
    steps: [
      ['camera-identity', 'Identified', 'camera-identity'],
      ['camera-snapshot', 'State dumped', 'camera-snapshot'],
      ['camera-menu', 'Menus driven', 'camera-menu'],
      ['camera-restart', 'Restarts itself', 'camera-restart'],
    ],
  },
  {
    id: 'fujimod',
    phase: 'mods',
    title: 'FujiMod',
    weight: 25,
    steps: [
      ['mod-target', 'Mod target', 'mod-target'],
      ['mod-build', 'Built', 'mod-build'],
    ],
  },
  {
    id: 'sd',
    phase: 'sd-card',
    title: 'SD card and install',
    weight: 10,
    steps: [
      [
        'boot-rom-loads-sd-card-on-camera',
        'Boot ROM loads an SD card',
        'boot-rom-loads-sd-card-on-camera',
      ],
      [
        'sd-boot-v1-fujimod-boots-on-camera',
        'FujiMod boots from an SD card',
        'sd-boot-v1-fujimod-boots-on-camera',
      ],
    ],
  },
];
function step(id: string, label: string, stage: string, result: 'pass' | 'fail' | 'not-run') {
  return {
    id,
    label,
    stage,
    result,
    passed: result === 'pass' ? 1 : 0,
    total: 1,
  };
}
function progress(
  model: string,
  passed: string[],
  mods: Record<string, 'pass' | 'fail'> = {},
  version = '1.00',
): FirmwareProgress {
  return {
    firmware_id: `${model.toLowerCase()}-100`,
    model,
    version,
    facts: null,
    boot_rom: null,
    tracks: TRACKS.map((track) => ({
      id: track.id,
      title: track.title,
      phase: track.phase,
      weight: track.weight,
      cells: track.cells ?? false,
      stages: [],
      steps: [
        ...track.steps.map(([id, label, stage]) =>
          step(id, label, stage, passed.includes(id) ? 'pass' : 'not-run'),
        ),
        ...(track.id === 'fujimod'
          ? Object.entries(mods).map(([mod, result]) =>
              step('walkthroughs/' + mod, mod, 'walkthroughs', result),
            )
          : []),
      ],
    })),
    stock: {},
    mods,
  };
}
function result(document: FirmwareProgress, id: string, value: 'stale' | 'fail') {
  for (const entry of document.tracks)
    for (const step of entry.steps)
      if (step.id === id) {
        step.result = value;
        step.passed = 0;
        step.total = 1;
      }
}
function table(
  columns: Parameters<typeof CameraTable>[0]['columns'],
  documents: FirmwareProgress[],
  owners: Owner[] = [],
) {
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => <CameraTable columns={columns} progress={byModel(documents)} owners={owners} />,
    host,
  );
  flush();
  return host;
}

test('each phase fills by its steps and names its value', () => {
  const document = progress(
    'X-E2S',
    ['research', 'rec', 'camera-identity', 'mod-target', 'mod-build'],
    {
      a: 'pass',
      b: 'fail',
      c: 'fail',
      d: 'fail',
    },
  );
  const analysis = document.tracks[0]!.steps.find((step) => step.id === 'analysis')!;
  analysis.result = 'not-run';
  analysis.passed = 1;
  analysis.total = 2;
  analysis.detail = '1 of 2 facts';
  const boot = document.tracks[0]!.steps.find((step) => step.id === 'boot-rom')!;
  boot.result = 'pass';
  boot.passed = 1;
  boot.total = 1;
  boot.detail = 'X70 dump';
  const list = phases(document);
  expect(list.map((phase) => [phase.id, phase.fraction, phase.value])).toEqual([
    ['research', (1 + 0.5 + 1) / 3, '2 of 3'],
    ['emulator', 1 / 6, '1 of 6'],
    ['camera', 1 / 4, '1 of 4'],
    ['mods', 3 / 6, '3 of 6'],
    ['sd-card', 0, 'Not yet'],
  ]);
  expect(list[0]?.steps.map((step) => [step.label, step.fraction, step.detail])).toEqual([
    ['Update file registered', 1, undefined],
    ['Facts located', 1 / 2, '1 of 2 facts'],
    ['Boot ROM', 1, 'X70 dump'],
  ]);
});

test('the emulator draws one cell per feature the body has, partial ones by their share', () => {
  const document = progress('X-T4', ['rec']);
  const emulator = document.tracks[1]!.steps;
  const feature = (id: string) => emulator.find((step) => step.id === 'stock/' + id)!;
  Object.assign(feature('film-simulations'), {
    result: 'partial',
    passed: 9,
    total: 11,
    detail: '9 of 11',
  });
  Object.assign(feature('ibis'), { result: 'pass', passed: 1, total: 1 });
  Object.assign(feature('sd'), { result: 'stale', passed: 0, total: 1 });
  const cellsPhase = phases(document)[1]!;
  expect(cellsPhase.cells).toBe(true);
  expect(cellsPhase.steps.map((step) => [step.label, step.fraction, step.detail])).toEqual([
    ['Boot', 1, undefined],
    ['Menus', 0, undefined],
    ['Film simulations', 9 / 11, '9 of 11'],
    ['IBIS', 1, undefined],
    ['SD', 0, undefined],
    ['Updater', 0, undefined],
  ]);
  const host = table([[{ name: 'X-Processor 4', years: '', models: ['X-T4'] }]], [document]);
  const cells = [...host.querySelectorAll<HTMLElement>('.phase:nth-child(2) .cell')];
  expect(cells.map((cell) => cell.style.getPropertyValue('--p'))).toEqual([
    '100%',
    '0%',
    `${(9 / 11) * 100}%`,
    '100%',
    '0%',
    '0%',
  ]);
  expect(host.querySelector('.phase:nth-child(2) li.partial .step-detail')?.textContent).toBe(
    '9 of 11',
  );
  expect(host.querySelector('.progress-heading')?.textContent).toBe('X-T41.00');
});

test('stale and failing stages do not count', () => {
  const document = progress('X-T10', ['rec']);
  result(document, 'menu', 'stale');
  result(document, 'research', 'fail');
  const [research, emulator] = phases(document);
  expect([research?.fraction, emulator?.fraction]).toEqual([0, 1 / 6]);
});

/** Mark stages inapplicable the way the generator does: the stage says so and writes no steps. */
function inapplicable(document: FirmwareProgress, stages: string[]) {
  for (const track of document.tracks) {
    track.stages.push(
      ...stages
        .filter((stage) => track.steps.some((step) => step.stage === stage))
        .map((stage) => ({ id: stage, title: stage, result: 'inapplicable' as const })),
    );
    track.steps = track.steps.filter((step) => !stages.includes(step.stage));
  }
}

test('steps of inapplicable stages are not written, so their track can still be done', () => {
  const document = progress('X-E4', ['boot-rom-loads-sd-card-on-camera', 'camera-identity']);
  inapplicable(document, ['sd-boot-v1-fujimod-boots-on-camera', 'camera-menu']);
  expect(document.tracks[4]!.stages).toEqual([
    {
      id: 'sd-boot-v1-fujimod-boots-on-camera',
      title: 'sd-boot-v1-fujimod-boots-on-camera',
      result: 'inapplicable',
    },
  ]);
  const [, , camera, , sd] = phases(document);
  expect([camera?.fraction, camera?.value, camera?.steps.length]).toEqual([1 / 3, '1 of 3', 3]);
  expect([sd?.fraction, sd?.value, sd?.steps.length]).toEqual([1, 'Done', 1]);
});

test('a track whose every stage is inapplicable is not applicable', () => {
  const document = progress('X-E4', []);
  inapplicable(document, ['research', 'analysis']);
  const [research] = phases(document);
  expect([research?.fraction, research?.value, research?.steps]).toEqual([1, 'Not applicable', []]);
});

test('a model is started once any stage, stock feature or mod passes', () => {
  const partial = progress('X30', []);
  const film = partial.tracks[1]!.steps.find((step) => step.id === 'stock/film-simulations')!;
  Object.assign(film, { result: 'partial', passed: 1, total: 2, detail: '1 of 2' });
  expect(started(partial)).toBe(true);
  Object.assign(film, { result: 'fail', passed: 0, total: 1, detail: undefined });
  expect(started(partial)).toBe(false);
  expect(started(progress('X30', []))).toBe(false);
  expect(started(progress('X30', [], { a: 'fail' }))).toBe(false);
  expect(started(progress('X30', [], { a: 'pass' }))).toBe(true);
  expect(started(progress('X30', ['camera-snapshot']))).toBe(true);
  expect(started(progress('X30', ['research']))).toBe(true);
  expect(started(progress('X30', ['analysis']))).toBe(true);
});

test('the newest firmware version with progress represents its model', () => {
  const documents = byModel([
    progress('X-M1', ['research'], {}, '1.50'),
    progress('X-M1', ['research'], {}, '1.20'),
    progress('X-M1', ['research'], {}, '1.5'),
  ]);
  expect(documents['X-M1']?.version).toBe('1.50');
});

test('started models draw a bar; the others are listed as not started', () => {
  const host = table(
    [
      [
        {
          name: 'EXR Processor II',
          years: '2013–2016',
          models: ['X70', 'X30', 'X20'],
        },
      ],
    ],
    [progress('X70', ['research', 'rec']), progress('X30', [], { a: 'fail' })],
  );
  expect([...host.querySelectorAll('.progress-row .name')].map((row) => row.textContent)).toEqual([
    'X70',
  ]);
  expect(host.querySelector('.rest')?.textContent).toBe('Not startedX30 · X20');
  expect(host.querySelectorAll('.progress-row .phase')).toHaveLength(5);
});

test('one tip is open at a time, whether tapped, hovered or focused', () => {
  const host = table(
    [[{ name: 'EXR Processor II', years: '', models: ['X70', 'X30'] }]],
    [progress('X70', ['research']), progress('X30', ['research'])],
  );
  const [a, b] = [...host.querySelectorAll<HTMLElement>('.bar')].map((bar) =>
    bar.querySelector<HTMLElement>('.phase:nth-child(2)')!,
  );
  const open = () => [...host.querySelectorAll('.phase.open')];
  const tap = (element: Element) => {
    element.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }));
    element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    flush();
  };
  tap(a!);
  expect(open()).toEqual([a]);
  tap(b!);
  expect(open()).toEqual([b]);
  tap(b!);
  expect(open()).toEqual([]);
  a!.dispatchEvent(new PointerEvent('pointerenter', { pointerType: 'mouse' }));
  flush();
  expect(open()).toEqual([a]);
  b!.focus();
  flush();
  expect(open()).toEqual([b]);
  document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  flush();
  expect(open()).toEqual([]);
});

test('the camera popover asks an owner for help until every camera step passes', () => {
  const host = table(
    [
      [
        {
          name: 'EXR Processor II',
          years: '',
          models: ['X-E2', 'X-E2S', 'X70'],
        },
      ],
    ],
    [
      progress('X-E2', ['research', 'rec']),
      progress('X-E2S', ['research', 'rec', 'camera-identity']),
      progress('X70', [
        'research',
        'rec',
        'camera-identity',
        'camera-snapshot',
        'camera-menu',
        'camera-restart',
      ]),
    ],
  );
  const help = [...host.querySelectorAll('.bar')].map(
    (bar) => bar.querySelector('.phase:nth-child(3) .help')?.textContent,
  );
  expect(help).toEqual(['Have an X-E2? Help test it', 'Help test on your X-E2S', undefined]);
  expect(host.querySelector('.phase:nth-child(3) .tip-value')?.textContent).toBe('Needs an owner');
});

test('a started model missing from the generation list appears under Other cameras', () => {
  const host = table(
    [[{ name: 'EXR Processor II', years: '', models: ['X70'] }]],
    [progress('X-A2', ['research', 'rec'])],
  );
  const groups = [...host.querySelectorAll('.gen h4')].map((heading) => heading.textContent);
  expect(groups).toEqual(['EXR Processor II', 'Other cameras']);
  expect(host.querySelector('.progress-row .name')?.textContent).toBe('X-A2');
});

test('bodies a contributor owns are outlined and the camera popover names the owners', () => {
  const host = table(
    [
      [
        {
          name: 'EXR Processor II',
          years: '',
          models: ['X70', 'X30', 'X100V', 'X20'],
        },
      ],
    ],
    [progress('X70', ['research']), progress('X30', ['research'])],
    [
      { name: 'One', github: 'one', bodies: ['X70', 'X100V'] },
      { name: 'Two', github: 'two', bodies: ['X70'] },
    ],
  );
  const rows = [...host.querySelectorAll('.progress-row')].map((row) => [
    row.querySelector('.name')?.textContent,
    row.classList.contains('owned'),
  ]);
  expect(rows).toEqual([
    ['X70', true],
    ['X30', false],
  ]);
  expect([...host.querySelectorAll('.rest .owned')].map((name) => name.textContent)).toEqual([
    'X100V',
  ]);
  expect(host.querySelector('.rest')?.textContent).toBe('Not startedX100V · X20');
  expect(host.querySelector('.legend .owned')?.textContent).toBe('Owned by a contributor');
  const [owned, unowned] = [...host.querySelectorAll('.bar .phase:nth-child(3)')];
  expect(owned?.querySelector('.tip-value')?.textContent).toBe('Not yet');
  expect(
    [...(owned?.querySelectorAll('.owners a') ?? [])].map((link) => [
      link.textContent,
      link.getAttribute('href'),
    ]),
  ).toEqual([
    ['@one', 'https://github.com/one'],
    ['@two', 'https://github.com/two'],
  ]);
  expect(owned?.querySelector('.help')?.textContent).toBe('Have an X70? Help test it');
  expect(unowned?.querySelector('.tip-value')?.textContent).toBe('Needs an owner');
  expect(unowned?.querySelector('.owners')).toBeNull();
});

test('a complete override publishes every phase and step as done', () => {
  const [document] = withOverrides(
    [progress('X70', ['research'], { a: 'pass', b: 'fail' })],
    [{ firmware_id: 'x70-100', complete: true }],
  );
  const list = phases(document!);
  expect(list.every((phase) => phase.fraction === 1)).toBe(true);
  expect(list.every((phase) => phase.steps.every((step) => step.fraction === 1))).toBe(true);
  expect(list.map((phase) => phase.value)).toEqual(['Done', 'Done', 'Done', '2 of 2', 'Done']);
  expect(started(document!)).toBe(true);
  expect(() => withOverrides([], [{ firmware_id: 'x70-100', complete: true }])).toThrow(
    'unknown firmware',
  );
});
