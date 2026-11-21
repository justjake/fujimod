import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test } from 'vite-plus/test';
import { byModel, phases, started, type FirmwareProgress, type TrackId } from './camera-progress';
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
  steps: [string, string, string][];
}[] = [
  {
    id: 'static',
    phase: 'research',
    title: 'Static analysis',
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
    steps: [
      ['mod-target', 'Mod target', 'mod-target'],
      ['mod-build', 'Built', 'mod-build'],
    ],
  },
  {
    id: 'sd',
    phase: 'sd-card',
    title: 'SD card and install',
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
    milestone: true,
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

test('each graph track reserves space for all of its tagged milestones', () => {
  const document = progress('X-E2S', [
    'research',
    'rec',
    'camera-identity',
    'mod-target',
    'mod-build',
  ]);
  const analysis = document.tracks[0]!.steps.find((step) => step.id === 'analysis')!;
  const boot = document.tracks[0]!.steps.find((step) => step.id === 'boot-rom')!;
  Object.assign(analysis, { result: 'partial', passed: 99, total: 100, detail: '99 of 100 facts' });
  Object.assign(boot, { result: 'pass', milestone: false });
  result(document, 'menu', 'stale');
  result(document, 'sd-boot-v1-fujimod-boots-on-camera', 'fail');
  const groups = phases(document);
  expect(groups.map((group) => [group.id, group.passed, group.steps.length])).toEqual([
    ['research', 1, 2],
    ['emulator', 1, 6],
    ['camera', 1, 4],
    ['mods', 2, 2],
    ['sd-card', 0, 2],
  ]);
  const host = table([[{ name: 'EXR Processor II', years: '', models: ['X-E2S'] }]], [document]);
  const bar = host.querySelector('[role="progressbar"]');
  expect(bar?.getAttribute('aria-valuenow')).toBe('5');
  expect(bar?.getAttribute('aria-valuemax')).toBe('16');
  const segments = [...host.querySelectorAll<HTMLElement>('.track-group')];
  expect(segments.map((segment) => segment.style.getPropertyValue('--outcomes'))).toEqual([
    '2',
    '6',
    '4',
    '2',
    '2',
  ]);
  expect(
    segments.map((segment) => Number.parseFloat(segment.style.getPropertyValue('--p'))),
  ).toEqual([50, (1 / 6) * 100, 25, 100, 0]);
});

test('a released badge leaves the engineering count as recorded', () => {
  const camera = progress('X70', ['rec']);
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => (
      <CameraTable
        columns={[[{ name: 'EXR Processor II', years: '', models: ['X70'] }]]}
        progress={byModel([camera])}
        owners={[]}
        released={new Set([camera.firmware_id])}
      />
    ),
    host,
  );
  flush();
  expect(host.querySelector('.progress-heading')?.textContent).toContain('Released');
  expect(host.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('1');
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
  expect(host.querySelectorAll('.progress-row .progress-bar')).toHaveLength(1);
});

test('one tip is open at a time, whether tapped, hovered or focused', () => {
  const host = table(
    [[{ name: 'EXR Processor II', years: '', models: ['X70', 'X30'] }]],
    [progress('X70', ['research']), progress('X30', ['research'])],
  );
  const [a, b] = [...host.querySelectorAll<HTMLElement>('.bar')].map((bar) =>
    bar.querySelector<HTMLElement>('.progress-bar')!,
  );
  const open = () => [...host.querySelectorAll('.progress-bar.open')];
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
    (bar) => bar.querySelector('.help')?.textContent,
  );
  expect(help).toEqual(['Have an X-E2? Help test it', 'Help test on your X-E2S', undefined]);
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
  const [owned, unowned] = [...host.querySelectorAll('.bar .progress-bar')];
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
  expect(unowned?.querySelector('.owners')).toBeNull();
});
