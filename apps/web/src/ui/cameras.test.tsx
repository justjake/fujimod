import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test } from 'vite-plus/test';
import {
  STAGES,
  byModel,
  phases,
  started,
  type FirmwareProgress,
  type Stage,
  type StageProgress,
} from './camera-progress';
import { CameraTable, type Owner } from './cameras';
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
function progress(
  model: string,
  passed: Stage[],
  mods: Record<string, 'pass' | 'fail'> = {},
  version = '1.00',
): FirmwareProgress {
  const stages = Object.fromEntries(
    STAGES.map((stage): [Stage, StageProgress] => [
      stage,
      passed.includes(stage) ? 'pass' : 'not-run',
    ]),
  ) as Record<Stage, StageProgress>;
  return {
    firmware_id: `${model.toLowerCase()}-100`,
    model,
    version,
    facts: null,
    boot_rom: null,
    stages,
    stock: {},
    mods,
  };
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
  document.facts = { located: 1, total: 2 };
  document.boot_rom = 'X70';
  const list = phases(document);
  expect(list.map((phase) => [phase.id, phase.fraction, phase.value])).toEqual([
    ['research', (3 + 0.5 + 1) / 6, '4 of 6'],
    ['emulator', 1 / 2, '1 of 2'],
    ['camera', 1 / 3, '1 of 3'],
    ['mods', 1 / 4, '1 of 4'],
    ['sd-card', 0, 'Not yet'],
  ]);
  expect(list[0]?.steps.map((step) => [step.label, step.fraction, step.detail])).toEqual([
    ['Update file registered', 1, undefined],
    ['Overlays unpacked', 1, undefined],
    ['Corpus exported', 1, undefined],
    ['Function index built', 0, undefined],
    ['Facts located', 1 / 2, '1 of 2'],
    ['Boot ROM', 1, 'X70 dump'],
  ]);
});

test('the emulator draws one cell per feature the body has, partial ones by their share', () => {
  const document = progress('X-T4', ['rec']);
  document.stock = {
    'film-simulations': { result: 'partial', passed: 9, total: 11 },
    ibis: { result: 'pass' },
    sd: { result: 'stale' },
  };
  const emulator = phases(document)[1]!;
  expect(emulator.cells).toBe(true);
  expect(emulator.steps.map((step) => [step.label, step.fraction, step.detail])).toEqual([
    ['Boot', 1, undefined],
    ['Menus', 0, undefined],
    ['Film simulations', 9 / 11, '9 of 11'],
    ['IBIS', 1, undefined],
    ['SD', 0, undefined],
  ]);
  const host = table([[{ name: 'X-Processor 4', years: '', models: ['X-T4'] }]], [document]);
  const cells = [...host.querySelectorAll<HTMLElement>('.phase:nth-child(2) .cell')];
  expect(cells.map((cell) => cell.style.getPropertyValue('--p'))).toEqual([
    '100%',
    '0%',
    `${(9 / 11) * 100}%`,
    '100%',
    '0%',
  ]);
  expect(host.querySelector('.phase:nth-child(2) li.partial .step-detail')?.textContent).toBe(
    '9 of 11',
  );
  expect(host.querySelector('.progress-heading')?.textContent).toBe('X-T41.00');
});

test('stale and failing stages do not count', () => {
  const document = progress('X-T10', ['rec']);
  document.stages.menu = 'stale';
  document.stages.research = 'fail';
  const [research, emulator] = phases(document);
  expect([research?.fraction, emulator?.fraction]).toEqual([0, 1 / 2]);
});

test('a model is started once any stage, stock feature or mod passes', () => {
  const partial = progress('X30', []);
  partial.stock = { lens: { result: 'partial', passed: 1, total: 2 } };
  expect(started(partial)).toBe(true);
  partial.stock = { lens: { result: 'fail' } };
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
    [[{ name: 'EXR Processor II', years: '2013–2016', models: ['X70', 'X30', 'X20'] }]],
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
    [[{ name: 'EXR Processor II', years: '', models: ['X-E2', 'X-E2S', 'X70'] }]],
    [
      progress('X-E2', ['research', 'rec']),
      progress('X-E2S', ['research', 'rec', 'camera-identity']),
      progress('X70', ['research', 'rec', 'camera-identity', 'camera-snapshot', 'camera-menu']),
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
    [[{ name: 'EXR Processor II', years: '', models: ['X70', 'X30', 'X100V', 'X20'] }]],
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
