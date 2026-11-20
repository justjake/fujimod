import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { Firmware } from '../catalog';
import type { CameraSupport } from '../home/state';
import { CameraLauncher } from './Launcher';
import type { FirmwareLibrary } from './firmware-library';
import type { LibraryFirmware } from './store';

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
function support(model: string, hash: string, mod: boolean, emu = true): CameraSupport {
  return {
    firmware: {
      id: model === 'X70' ? 'x70-110' : model.toLowerCase(),
      model,
      version: '1.10',
      sha256: hash,
      bytes: 4,
    } as Firmware,
    mod,
    emu,
  };
}
const entry: LibraryFirmware = {
  sha256: 'x70',
  firmwareId: 'x70-110',
  name: 'X70 · 1.10',
  fileName: 'FPUPDATE.DAT',
  addedAt: 1,
};
function mount(get: FirmwareLibrary['get']) {
  const run = vi.fn();
  const library = { busy: () => false, get } as unknown as FirmwareLibrary;
  const root = document.createElement('div');
  document.body.append(root);
  dispose = render(
    () => (
      <CameraLauncher
        ready
        supported={[support('X-M1', 'xm1', false), support('X70', 'x70', true)]}
        library={library}
        busy={false}
        message=""
        error={false}
        run={run}
      />
    ),
    root,
  );
  return { run, button: () => document.querySelector<HTMLButtonElement>('.launch-fujimod') };
}

test('one button downloads the Fujimod camera firmware and starts it with Fujimod', async () => {
  let progress: ((received: number) => void) | undefined;
  let finish: (value: LibraryFirmware) => void = () => {};
  const get = vi.fn((firmware: Firmware, report?: (received: number) => void) => {
    progress = report;
    return new Promise<LibraryFirmware>((resolve) => (finish = resolve));
  });
  const { run, button } = mount(get);
  expect(button()?.textContent).toBe('Launch Fujimod');
  button()!.click();
  flush();
  expect(get.mock.calls[0][0].model).toBe('X70');
  progress!(2);
  flush();
  expect(document.querySelector('progress')?.value).toBe(2);
  expect(document.querySelector('.launch-progress')?.textContent).toContain(
    'Downloading X70 1.10 firmware',
  );
  finish(entry);
  await Promise.resolve();
  await Promise.resolve();
  expect(run).toHaveBeenCalledWith(entry, true, false);
});

test('a failed download shows its error and offers the button again', async () => {
  const { run, button } = mount(() => Promise.reject(Error('Firmware download failed (404).')));
  button()!.click();
  await Promise.resolve();
  await Promise.resolve();
  flush();
  expect(document.querySelector('.launch-error')?.textContent).toBe(
    'Firmware download failed (404).',
  );
  expect(button()?.disabled).toBe(false);
  expect(run).not.toHaveBeenCalled();
});
