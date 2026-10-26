import { expect, test } from 'vite-plus/test';
import type { PhysicalControls, PhysicalControl } from 'fujimod';
import { controlPart, heldControlPart, marking } from './control-part';
import { bootSequence } from './power-combo';

const documents = import.meta.glob<PhysicalControls>('../../../../catalog/bodies/*/controls.json', {
  eager: true,
  import: 'default',
});
const x70 = Object.values(documents).find((d) => d.body_id === 'x70')!;
const control = (id: string) => x70.controls[id]!;

for (const body of Object.values(documents)) {
  test(`${body.body_id}: every boot hold has a declared drawing`, () => {
    for (const combination of Object.values(body.boot ?? {})) {
      for (const hold of combination!.hold)
        expect(heldControlPart(body, hold.id, hold.stage ?? undefined)).toBeDefined();
    }
  });
}

test('each mechanism maps to its drawing and held part', () => {
  expect(controlPart(control('movie_record_button'))).toEqual({
    kind: 'key',
    marking: { icon: 'record' },
  });
  expect(controlPart(control('shutter_button'), undefined, 'full')).toMatchObject({
    kind: 'shutter',
    stage: 'full',
  });
  expect(controlPart(control('command_control'), 'press')).toEqual({
    kind: 'rocker',
    marking: { shortName: 'COMMAND' },
    held: 'press',
  });
  for (const action of ['decrease', 'increase'])
    expect(controlPart(control('command_control'), action)).toMatchObject({
      kind: 'rocker',
      held: action,
    });
  expect(controlPart(control('selector'), 'up')).toMatchObject({ kind: 'pad', held: 'up' });
  expect(controlPart(control('focus_mode_selector'), 'M')).toMatchObject({
    kind: 'positions',
    position: 'M',
  });
  expect(controlPart(control('auto_mode_selector_lever'))).toMatchObject({ kind: 'positions' });
  expect(controlPart(control('aperture_ring'), 'A')).toMatchObject({
    kind: 'positions',
    position: 'A',
  });
  expect(controlPart(control('shutter_speed_dial'), 'A')).toMatchObject({
    kind: 'positions',
    position: 'A',
  });
  for (const kind of ['dial', 'ring'] as const) {
    const rotating: PhysicalControl = {
      ...control('command_control'),
      behavior: {
        kind,
        motion: { kind: 'steps', decrease: null, increase: null },
        press: null,
        endless: true,
      },
    };
    expect(controlPart(rotating, 'increase')).toMatchObject({ kind: 'command', held: 'increase' });
  }
  expect(marking(control('disp_back_button'))).toEqual({ face: ['DISP', 'BACK'] });
});

test('unlocated pushable centers are admitted; undeclared actions and ids are refused', () => {
  const command = structuredClone(control('command_control'));
  if (command.behavior?.kind !== 'rocker') throw Error('Expected rocker');
  command.behavior.press = null;
  expect(controlPart(command, 'press')).toMatchObject({ held: 'press' });
  command.pushable = false;
  expect(() => controlPart(command, 'press')).toThrow();
  for (const [id, action, stage] of [
    ['movie_record_button', 'up'],
    ['command_control', 'left'],
    ['selector', 'diagonal'],
    ['focus_mode_selector', 'Z'],
    ['shutter_button', undefined, 'invalid'],
    ['movie_record_button', undefined, 'full'],
    ['touchscreen'],
    ['front_ring'],
  ])
    expect(() => controlPart(control(id!), action, stage)).toThrow();
  expect(() => heldControlPart(x70, 'missing.press')).toThrow('Unknown control id');
  expect(() => heldControlPart(x70, 'selector.up.extra')).toThrow('Unknown control id');
});

test('boot sequences join holds in order and reject missing modes and USB conflicts', () => {
  expect(bootSequence(x70, ['sd', 'firmware_update', 'sd'])).toEqual({
    hold: [
      { id: 'command_control.press' },
      { id: 'movie_record_button' },
      { id: 'disp_back_button' },
    ],
  });
  const changed = structuredClone(x70);
  changed.boot!.sd!.usb = false;
  expect(() => bootSequence(changed, ['sd', 'service'])).toThrow('USB');
  delete changed.boot!.sd;
  expect(() => bootSequence(changed, ['sd'])).toThrow('No sd');
});
