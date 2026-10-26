import type { PhysicalControls } from 'fujimod';
import { marking } from './control-part';
import type { ControlInput } from 'fujimod/emulator';
import { expect, test } from 'vite-plus/test';
import { compatibleVariants, defaultLayout } from './default-layout';
import type { ControlNode } from './layout';
import catalog from './fixtures/catalog-inputs.json';

const documents = import.meta.glob<PhysicalControls>('../../../../catalog/bodies/*/controls.json', {
  eager: true,
  import: 'default',
});
const physical = (id: string) => {
  const body = id.split('-')[0] === 'xq1' || id.split('-')[0] === 'xq2' ? 'xq' : id.split('-')[0];
  const document = Object.values(documents).find((d) => d.body_id === body);
  if (!document) throw new Error(`No physical controls for ${id}`);
  return document;
};
const bodies = catalog as Record<string, ControlInput[]>;

function nodes(layout: ControlNode[]): ControlNode[] {
  return layout.flatMap((node) => [node, ...nodes(node.children ?? [])]);
}

function boundIds(layout: ControlNode[]): string[] {
  return nodes(layout).flatMap((node) => [
    ...(node.id ? [node.id] : []),
    ...Object.values(node.bindings ?? {}),
  ]);
}

test('an empty input list gives an empty panel', () => {
  expect(defaultLayout([], physical('x70'))).toEqual({
    schema: 1,
    body: 'x70',
    combos: [],
    layout: [],
  });
});

for (const [body, inputs] of Object.entries(bodies)) {
  test(`${body}: only supported controls, each once, with short faces`, () => {
    const panel = defaultLayout(inputs, physical(body));
    const ids = boundIds(panel.layout);
    const byId = new Map(inputs.map((input) => [input.id, input]));
    for (const id of ids) {
      const input = byId.get(id)!;
      expect(input, id).toBeTruthy();
      expect(input.supported, id).toBe(true);
      expect(input.kind, id).not.toBe('touch');
      expect(id.endsWith('_latch'), id).toBe(false);
    }
    expect(new Set(ids).size).toBe(ids.length);
    const shown = inputs.filter(
      (input) => input.supported && input.kind !== 'touch' && !input.id.endsWith('_latch'),
    );
    expect(ids.sort()).toEqual(shown.map((input) => input.id).sort());
    for (const node of nodes(panel.layout)) {
      for (const line of node.face ?? []) expect(line.length, line).toBeLessThanOrEqual(6);
      expect((node.face ?? []).length).toBeLessThanOrEqual(2);
      expect((node.caption ?? '').length, node.caption).toBeLessThanOrEqual(10);
    }
  });
}

test('direction groups become one D-pad, centered on press or MENU/OK', () => {
  const x20 = nodes(defaultLayout(bodies['x20-103']!, physical('x20')).layout).filter(
    (n) => n.kind === 'dpad',
  );
  expect(x20).toHaveLength(1);
  expect(x20[0]!.bindings!.center).toBe('selector.press');
  const x70 = nodes(defaultLayout(bodies['x70-110']!, physical('x70')).layout).filter(
    (n) => n.kind === 'dpad',
  );
  expect(x70).toHaveLength(1);
  expect(x70[0]!.bindings!.center).toBe('menu_ok_button');
  expect(x70[0]!.face).toEqual(['MENU', 'OK']);
});

test('step groups distinguish rockers from command dials', () => {
  const x70 = nodes(defaultLayout(bodies['x70-110']!, physical('x70')).layout).find(
    (n) => n.kind === 'rocker',
  );
  expect(x70!.bindings).toEqual({
    left: 'command_control.decrease',
    right: 'command_control.increase',
    center: 'command_control.press',
  });
  expect(x70!.caption).toBe('COMMAND');
  const xq1 = nodes(defaultLayout(bodies['xq1-121']!, physical('xq1')).layout);
  expect(
    xq1.some((node) => node.kind === 'command' && node.bindings?.left === 'zoom_control.wide'),
  ).toBe(false);
});

test('catalog facts supply all faces without reading the input labels', () => {
  const controls = physical('x70');
  expect(marking(controls.controls.playback_button!)).toEqual({ icon: 'playback' });
  expect(marking(controls.controls.movie_record_button!)).toEqual({ icon: 'record' });
  expect(marking(controls.controls.disp_back_button!)).toEqual({ face: ['DISP', 'BACK'] });
  const renamed = bodies['x70-110']!.map((input) => ({ ...input, label: 'Unrelated manual name' }));
  expect(defaultLayout(renamed, controls)).toEqual(defaultLayout(bodies['x70-110']!, controls));
});

test('relative motion, positions and stages decide presentation before kind', () => {
  const dial = bodies['xe2s-111']!.find((input) => input.id === 'command_dial')!;
  expect(dial.kind).toBe('button');
  expect(compatibleVariants(dial)[0]).toBe('ticks');
  const ev = bodies['x70-110']!.find((input) => input.id === 'exposure_compensation_dial')!;
  expect(compatibleVariants(ev)[0]).toBe('dial');
  const focus = bodies['x70-110']!.find((input) => input.id === 'focus_mode_selector')!;
  expect(compatibleVariants(focus)[0]).toBe('segment');
  const shutter = bodies['x70-110']!.find((input) => input.id === 'shutter_button')!;
  expect(compatibleVariants(shutter)).toEqual(['shutter']);
});
