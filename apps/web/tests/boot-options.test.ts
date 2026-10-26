import { expect, test } from 'vite-plus/test';
import type { PhysicalControls } from 'fujimod';
import x70 from '../../../catalog/bodies/x70/controls.json';
import xm5 from '../../../catalog/bodies/xm5/controls.json';
import { bootCombination, panelBootCombos } from '../src/cameras/boot-options';
import { serviceModeEntry } from '../src/dev-tools/service-mode-entry';

test('controls preserve the X70 startup inputs and service guide', () => {
  const combos = panelBootCombos(x70 as PhysicalControls, ['sd', 'firmware_update', 'service']);
  expect(combos.map((combo) => combo.inputs)).toEqual([
    [{ id: 'command_control.press' }, { id: 'movie_record_button' }],
    [{ id: 'disp_back_button' }],
    [{ id: 'usb', position: 'on' }, { id: 'selector.up' }, { id: 'shutter_button', stage: 'full' }],
  ]);
  expect(combos.map((combo) => combo.modes)).toEqual([['sd'], ['firmware_update'], ['service']]);
  expect(serviceModeEntry).toEqual([
    'Switch the camera OFF.',
    'Release the buttons when the green lamp lights.',
  ]);
});

test('X-M5 boots from SD with Q and the front command dial pressed, USB unplugged', () => {
  expect(bootCombination(xm5 as PhysicalControls, 'sd')).toEqual({
    usb: false,
    hold: [{ id: 'quick_menu_button' }, { id: 'front_command_dial.press' }],
  });
});
