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
      downloads: { sd_boot_v1: true, sd_boot_v1_installable: false },
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
  const get = vi.fn(async () => firmware);
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
    get,
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
  return { host, run, download, get };
}
test('camera selection enables Run before importing firmware', async () => {
  const { host, run, get } = mount();
  expect(host.querySelector('.release-gate')).toBeNull();
  expect(host.querySelector('.downloads')).toBeNull();
  const select = host.querySelector<HTMLSelectElement>('select[aria-label=Camera]')!;
  expect(select.value).toBe('x70');
  const actions = host.querySelectorAll<HTMLButtonElement>('.release-hit');
  expect([...actions].map((button) => button.disabled)).toEqual([true, false, false, false]);
  actions[2].click();
  await vi.waitFor(() => expect(run).toHaveBeenCalledWith(firmware, true, false));
  expect(get).toHaveBeenCalledWith(supported[2].firmware);
});
test('failed automatic downloads show an error and do not start a camera', async () => {
  const { host, run, get } = mount();
  get.mockRejectedValueOnce(Error('Mirror unavailable'));
  host.querySelectorAll<HTMLButtonElement>('.release-hit')[3].click();
  await vi.waitFor(() =>
    expect(host.querySelector('[role=status]')?.textContent).toBe('Mirror unavailable'),
  );
  expect(run).not.toHaveBeenCalled();
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
test('changing the selected camera changes emulator availability without importing files', () => {
  const { host, get } = mount();
  const select = host.querySelector<HTMLSelectElement>('select[aria-label=Camera]')!;
  select.value = 'xt4';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  flush();
  expect(
    [...host.querySelectorAll<HTMLButtonElement>('.release-hit')].every(
      (button) => button.disabled,
    ),
  ).toBe(true);
  expect(get).not.toHaveBeenCalled();
});
