import { render } from '@solidjs/web';
import { createSignal, flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { Firmware } from '../catalog';
import type { CameraSupport } from '../home/state';
import type { LoadedRelease } from '../patch/release';
import { CameraLauncher } from './Launcher';
import { latestCamera } from './selection';
import { CameraStore, type CameraRecord, type LibraryFirmware } from './store';
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
function support(model: string, hash: string, emu = true): CameraSupport {
  return {
    firmware: {
      id: model.toLowerCase(),
      model,
      version: '1.10',
      sha256: hash,
      bytes: 1,
      download_url: `https://example.com/${model}`,
      release_date: '2020-01-01',
      processor: 'EXR Processor II',
    } as Firmware,
    mod: hash === 'x70',
    emu,
  };
}
const supported = [
  support('X-M1', 'xm1'),
  support('X-T10', 'xt10'),
  support('X70', 'x70'),
  support('X-T4', 'xt4', false),
];
const firmware: LibraryFirmware = {
  sha256: 'x70',
  firmwareId: 'x70-110',
  name: 'X70 · 1.10',
  fileName: 'FPUPDATE.DAT',
  addedAt: 1,
};
const release: LoadedRelease = {
  schema: 2,
  release: '0.0.1',
  tag: '0.0.1',
  name: 'FujiMod 0.0.1',
  published_at: '2026-10-26T12:00:00Z',
  prerelease: true,
  assetBase: 'https://example.com/releases/0.0.1/',
  notesText: '# What’s new\n- **Faster** menus',
  patches: { x70: 'x70.fmpatch' },
  firmwares: [
    {
      ...supported[2].firmware,
      patch_sha256: 'patch',
      patch_bytes: 1,
      boot_instructions: ['SD'],
      mods: [],
      downloads: { sd_card: true, installable: false },
    },
  ],
};
function camera(id: string, modEnabled: boolean, used: number, firmwareHash = 'x70'): CameraRecord {
  return {
    id,
    name: id,
    firmwareId: firmwareHash,
    firmwareHash,
    fileName: 'FPUPDATE.DAT',
    createdAt: used - 1,
    savedAt: used,
    revision: 0,
    modEnabled,
  };
}
function mount(
  options: {
    selected?: boolean;
    cameras?: CameraRecord[];
    busy?: boolean;
    bodies?: CameraSupport[];
  } = {},
) {
  const [entries] = createSignal(options.selected ? [firmware] : []);
  const [selected, select] = createSignal(options.selected ? firmware.sha256 : undefined);
  const run = vi.fn(),
    download = vi.fn(async () => {}),
    add = vi.fn(async () => {}),
    remove = vi.fn(async () => {});
  const library = {
    store: new CameraStore(),
    entries,
    selected,
    chosen: () => entries().find((entry) => entry.sha256 === selected()),
    ready: () => true,
    busy: () => false,
    error: () => '',
    select,
    add,
    remove,
  };
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => (
      <CameraLauncher
        ready
        cameras={options.cameras ?? []}
        library={library}
        release={release}
        supported={options.bodies ?? supported}
        busy={options.busy ?? false}
        message=""
        error={false}
        run={run}
        download={download}
      />
    ),
    host,
  );
  flush();
  return { host, run, download };
}
test('empty state has four disabled actions, a firmware gate, shared picker and inline Moddable badge', () => {
  const { host } = mount();
  expect(host.querySelector('.release-gate')?.textContent).toBe('Add base firmware to continue');
  const actions = host.querySelectorAll<HTMLButtonElement>('.release-hit');
  expect(actions).toHaveLength(4);
  expect([...actions].every((button) => button.disabled)).toBe(true);
  expect(host.querySelectorAll('.launcher-row > .i')).toHaveLength(0);
  expect(host.querySelector('.release-notes strong')?.textContent).toBe('Faster');
  expect(host.querySelector('.file-drop-highlighted')).not.toBeNull();
  expect(host.querySelector('#firmware-file')).not.toBeNull();
  expect([...host.querySelectorAll('.downloads-model')].map((node) => node.textContent)).toEqual([
    'X70Moddable',
    'X-T10',
    'X-M1',
  ]);
  expect(host.querySelector('.downloads-camera-tag')).toBeNull();
});
test('selected firmware gates downloads independently from emulator support', () => {
  const { host, run } = mount({ selected: true });
  const buttons = host.querySelectorAll<HTMLButtonElement>('.release-hit');
  expect([...buttons].map((button) => button.disabled)).toEqual([true, false, false, false]);
  expect(host.querySelectorAll('.launcher-row')[0].textContent).toContain(
    'Not available for this camera yet',
  );
  expect(host.querySelectorAll('.launcher-row')[1].textContent).toContain(
    'X70 • 1.10 base firmware',
  );
  buttons[2].click();
  expect(run).toHaveBeenLastCalledWith(firmware, true, false);
  buttons[3].click();
  expect(run).toHaveBeenLastCalledWith(firmware, false, false);
  expect(host.querySelector('.release-gate')).toBeNull();
});
test('resume selects the latest camera with exactly the firmware and mod setting', () => {
  const cameras = [
    camera('old', true, 2),
    camera('stock', false, 10),
    camera('recent', true, 5),
    camera('different', true, 20, 'xm1'),
  ];
  expect(latestCamera(cameras, 'x70', true)?.id).toBe('recent');
  expect(latestCamera(cameras, 'x70', false)?.id).toBe('stock');
  expect(latestCamera(cameras, 'xt10', true)).toBeUndefined();
  const { host, run } = mount({ selected: true, cameras });
  const fresh = host.querySelectorAll<HTMLButtonElement>('.start-fresh');
  expect(fresh).toHaveLength(2);
  fresh[0].click();
  expect(run).toHaveBeenLastCalledWith(firmware, true, true);
  fresh[1].click();
  expect(run).toHaveBeenLastCalledWith(firmware, false, true);
  expect(host.querySelector('.resume-camera, .new-camera')).toBeNull();
});
test('busy actions cannot run or start fresh', () => {
  const { host } = mount({ selected: true, busy: true, cameras: [camera('saved', false, 1)] });
  expect(
    [...host.querySelectorAll<HTMLButtonElement>('.release-hit, .start-fresh')].every(
      (button) => button.disabled,
    ),
  ).toBe(true);
});
test('search filters a long list without breaking processor labels', () => {
  const { host } = mount({
    bodies: Array.from({ length: 50 }, (_, n) => support(`X-T${n + 1}`, `t${n}`)),
  });
  const search = host.querySelector<HTMLInputElement>('input[type=search]')!;
  search.value = 'x-t40';
  search.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flush();
  expect(host.querySelectorAll('.downloads-camera')).toHaveLength(1);
  expect(host.querySelector('.downloads-model')?.textContent).toBe('X-T40');
  search.value = 'gfx';
  search.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flush();
  expect(host.querySelector('.downloads-empty')?.textContent).toContain('gfx');
});
