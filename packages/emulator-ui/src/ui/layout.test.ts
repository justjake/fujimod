import { expect, test } from 'vite-plus/test';
import { exportDevicePanel, importDevicePanel, type DevicePanel } from './layout';

const panel = {
  schema: 1,
  body: 'x70',
  detentSounds: { command: 'compact' },
  layout: [
    { kind: 'control', id: 'shutter', variant: 'shutter' },
    {
      kind: 'dpad',
      bindings: { up: 'selector.up', center: 'menu_ok', left: 'missing-control' },
      children: [{ kind: 'control', id: 'missing-control', variant: 'key' }],
    },
  ],
  combos: [
    {
      label: 'SD card boot',
      modes: ['sd'],
      inputs: [{ id: 'command.press' }, { id: 'missing-combo-input', stage: 'half' }],
    },
  ],
} satisfies DevicePanel;

test('exports and imports the versioned panel tree for its body', () => {
  const json = exportDevicePanel(panel);
  const imported = importDevicePanel(json, 'x70', [
    { id: 'shutter' },
    { id: 'selector.up' },
    { id: 'menu_ok' },
    { id: 'command.press' },
  ]);

  expect(JSON.parse(json)).toEqual(panel);
  expect(imported).toEqual({ panel, unknownInputIds: ['missing-control', 'missing-combo-input'] });
});

test('reports each unknown bound ID once, including direct controls, bindings and boot inputs', () => {
  const imported = importDevicePanel(exportDevicePanel(panel), 'x70', [{ id: 'shutter' }]);

  expect(imported.unknownInputIds).toEqual([
    'selector.up',
    'menu_ok',
    'missing-control',
    'command.press',
    'missing-combo-input',
  ]);
});

test('rejects unsupported schema versions and body mismatches', () => {
  const wrongVersion = JSON.stringify({ ...panel, schema: 2 });

  expect(() => importDevicePanel(wrongVersion, 'x70', [])).toThrow(
    'Invalid device panel JSON or unsupported schema version.',
  );
  expect(() => importDevicePanel(exportDevicePanel(panel), 'xm1', [])).toThrow(
    'Device panel body mismatch: expected xm1, received x70.',
  );
});

test('rejects malformed layout nodes instead of importing a partial tree', () => {
  const malformed = JSON.stringify({ ...panel, layout: [{ kind: 'unknown-control' }] });

  expect(() => importDevicePanel(malformed, 'x70', [])).toThrow(
    'Invalid device panel JSON or unsupported schema version.',
  );
});
