import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { ResolvedSelection } from 'fujimod';
import { createTestCameraModel, testCamera } from '../test-camera-model';
import type { CaptureSet } from './capture';

const selection = {
  firmware: { id: 'xm5-130', version: '1.30' },
  physical_controls: {
    controls: {
      q_button: { name: 'Q button', location: 'rear', mechanical: false },
      drive_lever: { name: 'Drive lever', location: 'top', mechanical: false },
    },
  },
  bulk: {
    product: 'X-M5',
    internal_version: '2.06',
    scopes: [
      { name: 'quick', basis: '', ranges: [{ name: 'firmware', address: 0x1000, size: 8 }] },
      { name: 'full', basis: '', ranges: [{ name: 'ram', address: 0x2000, size: 16 }] },
    ],
  },
} as unknown as ResolvedSelection;
vi.mock('fujimod', () => ({ catalog: { selections: () => [selection] } }));
const { StateCaptures } = await import('./StateCaptures');
const { MapControls } = await import('./MapControls');

let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  document.body.innerHTML = '';
});
const button = (host: HTMLElement, name: string) =>
  [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === name)!;

function mount(component: typeof StateCaptures, reads: () => Uint8Array) {
  const calls: [string, number, number][] = [];
  const test = createTestCameraModel({
    session: () => ({
      async readBulk(firmware, address, size) {
        calls.push([firmware, address, size]);
        return { data: reads().slice(0, size), overwritten: { address: 0x9000, size: 4 } };
      },
    }),
  });
  test.add(testCamera('m5', { identity: { product: 'X-M5', internalVersion: '2.06' } }));
  let set: CaptureSet | undefined;
  const files: { name: string; blob: Blob; meta?: Record<string, unknown> }[] = [];
  const host = document.createElement('div');
  document.body.append(host);
  const Component = component;
  const view = () => (
    <Component
      camera={test.model.cameras()[0]}
      model={test.model}
      set={set}
      onSet={(next) => (set = next)}
      onFile={(name, blob, meta) => files.push({ name, blob, meta })}
      onConnect={() => {}}
      onClose={() => {}}
    />
  );
  cleanup = render(view, host);
  return { host, calls, set: () => set, files };
}

test('a state capture reads the largest scope and adds each range as a file', async () => {
  const { host, calls, set, files } = mount(StateCaptures, () =>
    Uint8Array.of(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16),
  );
  flush();
  expect(host.querySelector('.cap-head')!.textContent).toContain('X-M5 1.30');
  expect(host.querySelector('[role="radiogroup"]')).toBeNull();
  const row = [...host.querySelectorAll('.cap-row')].find((node) =>
    node.textContent?.startsWith('AF-S half press'),
  )!;
  row.querySelector('button')!.click();
  await vi.waitFor(() => expect(set()?.states.length).toBe(1));
  expect(calls).toEqual([['xm5-130', 0x2000, 16]]);
  const [capture] = set()!.states;
  expect(capture.label).toBe('AF-S half press');
  expect(capture.overwritten).toEqual([{ address: 0x9000, size: 4 }]);
  expect(capture.ranges[0].pieces[0].sha256).toMatch(/^[0-9a-f]{64}$/);
  expect(files.map((file) => file.name)).toEqual(['capture-af-s-half-press-ram.bin']);
  expect(files[0]!.blob.size).toBe(16);
  expect(files[0]!.meta).toMatchObject({
    address: 0x2000,
    scope: 'full',
    label: 'AF-S half press',
  });
});

test('menu pages need a page name before capturing', () => {
  const { host } = mount(StateCaptures, () => new Uint8Array(8));
  flush();
  const row = [...host.querySelectorAll('.cap-row')].find((node) =>
    node.textContent?.startsWith('Menu page'),
  )!;
  expect(row.querySelector<HTMLButtonElement>('button')!.disabled).toBe(true);
});

test('mapping a control takes released, held, released, held captures', async () => {
  let held = false;
  const { host, set, files } = mount(MapControls, () =>
    Uint8Array.of(held ? 1 : 0, 0, 0, 0, 7, 0, 0, 0),
  );
  flush();
  button(host, 'Q buttonrear').click();
  flush();
  for (const step of ['Capture released', 'Capture held', 'Capture released', 'Capture held']) {
    await vi.waitFor(() => {
      flush();
      expect(button(host, step)?.disabled).toBe(false);
    });
    held = step === 'Capture held';
    button(host, step).click();
  }
  await vi.waitFor(() => expect(set()?.mappings.length).toBe(1));
  const [mapping] = set()!.mappings;
  expect(mapping.control).toBe('q_button');
  expect([...mapping.diff.candidates]).toEqual([0x1000]);
  expect(files.map((file) => file.name)).toEqual([
    'mapping-q-button-candidates.bin',
    'mapping-q-button-changed.bin',
  ]);
  expect([...new Uint32Array(await files[0]!.blob.arrayBuffer())]).toEqual([0x1000]);
});
