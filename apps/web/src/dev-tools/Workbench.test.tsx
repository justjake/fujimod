import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, beforeAll, expect, test, vi } from 'vite-plus/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { initialize } from 'fujimod';
import { Workbench } from './Workbench';
import { createTestCameraModel, testCamera } from './test-camera-model';
import { Direction, Layer, Level } from './log';

beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(resolve('../../bindings/javascript/pkg/fujimod_bg.wasm')),
  });
});

let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});
function mount(model: Parameters<typeof Workbench>[0]['model']) {
  const host = document.createElement('div');
  document.body.append(host);
  const dispose = render(() => <Workbench model={model} usbSupported />, host);
  cleanup = dispose;
  return host;
}
const button = (host: HTMLElement, name: string) =>
  [...host.querySelectorAll('button')].find(
    (node) => (node.getAttribute('aria-label') ?? node.textContent?.trim()) === name,
  )!;

test('firmware identity updates show public versions and a model-specific official update prompt', () => {
  const fixture = createTestCameraModel({});
  fixture.add(testCamera('camera', { identity: { product: 'X-T1', internalVersion: '2.30' } }));
  const host = mount(fixture.model);
  flush();
  expect(host.querySelector('.wb-firmware-update')!.textContent).toContain(
    'Unknown firmware version appears to be outdated.',
  );
  expect(host.querySelector<HTMLAnchorElement>('.wb-firmware-update a')!.href).toBe(
    'https://www.fujifilm-x.com/global/support/download/firmware/cameras/x-t1/',
  );
  fixture.update('camera', (camera) => ({
    ...camera,
    identity: { product: 'X-T1', internalVersion: '2.39' },
  }));
  flush();
  expect(host.querySelector('.wb-kv')!.textContent).toContain('5.52 · internal 2.39');
  expect(host.querySelector('.wb-firmware-update')).toBeNull();
  fixture.update('camera', (camera) => ({
    ...camera,
    identity: { product: 'X-M1', internalVersion: '1.61' },
  }));
  flush();
  expect(host.querySelector('.wb-kv')!.textContent).toContain('1.01 · internal 1.61');
  expect(host.querySelector('.wb-firmware-update')!.textContent).toContain(
    'Newer firmware is available.',
  );
});

test('the splash connects a camera and the workbench selects it', async () => {
  const test = createTestCameraModel({ next: () => testCamera('X100V') });
  const host = mount(test.model);
  flush();
  expect(host.querySelector('.td-enter h2')!.textContent).toBe('Enter service mode');
  host.querySelector<HTMLButtonElement>('button[aria-label="Connect camera"]')!.click();
  await vi.waitFor(() => {
    flush();
    expect(host.querySelector('.wb-camera h2')!.textContent).toBe('X100V');
  });
  expect(host.querySelector('.wb-cam.selected')!.textContent).toContain('X100V');
  expect(host.querySelector('.wb-state')!.textContent).toBe('Idle');
});

test('the camera column appears only once more than one camera is known', () => {
  const test = createTestCameraModel({});
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  test.append({
    camera: 'X100V',
    layer: Layer.Ff80,
    level: Level.Debug,
    direction: Direction.In,
    message: 'Device info · Product → X100V',
    sizes: { sent: 32, received: 128 },
  });
  flush();
  expect(host.querySelector('.cl-camera')).toBeNull();
  expect(host.querySelector('[aria-label="Log cameras"]')).toBeNull();
  expect(host.querySelector('.cl-size')!.textContent).toBe('32 / 128 B');
  test.add(testCamera('X70', { name: 'X70 simulator' }));
  flush();
  expect(host.querySelector('.cl-camera')!.textContent).toBe('X100V');
  const filter = [...host.querySelectorAll('[aria-label="Log cameras"] button')].find(
    (node) => node.textContent === 'X100V',
  ) as HTMLButtonElement;
  filter.click();
  flush();
  expect(host.querySelector('.cl-empty')!.textContent).toBe('No matching entries');
});

test('the Connection layer filters like every other layer', () => {
  const test = createTestCameraModel({});
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  test.append({
    camera: 'X100V',
    layer: Layer.Connection,
    level: Level.Info,
    direction: Direction.Host,
    message: 'Sharing started',
  });
  flush();
  expect(host.querySelector('.cl-message')!.textContent).toBe('Sharing started');
  button(host, 'Connection').click();
  flush();
  expect(host.querySelector('.cl-message')).toBeNull();
});

test('a camera held by a remote shows who holds it and disables every tool', () => {
  const test = createTestCameraModel({});
  test.add(
    testCamera('X100V', {
      session: { kind: 'remote', controller: 'peer-1', reason: 'fujimod-capture' },
    }),
  );
  const host = mount(test.model);
  flush();
  expect(host.querySelector('.wb-state')!.textContent).toBe('In use by fujimod-capture');
  for (const name of ['Dump configuration', 'Enable debug mode'])
    expect(button(host, name).disabled).toBe(true);
  expect(host.querySelector('[aria-labelledby="wb-memory"]')).toBeNull();
});

test('sharing is an Off/On control on the camera card and never disables the tools', async () => {
  const test = createTestCameraModel({});
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  const on = [...host.querySelectorAll('[aria-label="Sharing"] button')].find(
    (node) => node.textContent === 'On',
  ) as HTMLButtonElement;
  on.click();
  await vi.waitFor(() => {
    flush();
    expect(host.querySelector('.wb-tag')!.textContent).toBe('Shared · 0');
  });
  expect(test.sharingCalls).toEqual([['X100V', true]]);
  expect(button(host, 'Dump configuration').disabled).toBe(false);
});

test('Dump configuration adds configuration.bin to the Files pane', async () => {
  const pages: [number, number][] = [];
  const test = createTestCameraModel({
    session: () => ({
      configurationGeometry: async () => ({ capacity: 10, pageBytes: 4 }),
      readConfiguration: async (offset, length) => {
        pages.push([offset, length]);
        return new Uint8Array(length).fill(offset);
      },
    }),
  });
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  expect(host.querySelector('.files')!.textContent).toContain('No files yet');
  const blobs = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:file');
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  button(host, 'Dump configuration').click();
  await vi.waitFor(() => {
    flush();
    expect(host.querySelector('.files .tree')!.textContent).toContain('configuration.bin');
  });
  expect(pages).toEqual([
    [0, 4],
    [4, 4],
    [8, 2],
  ]);
  button(host, 'Download configuration.bin').click();
  await vi.waitFor(() => expect(blobs).toHaveBeenCalled());
  const bytes = new Uint8Array(await (blobs.mock.calls[0][0] as Blob).arrayBuffer());
  expect([...bytes]).toEqual([0, 0, 0, 0, 4, 4, 4, 4, 8, 8]);
  button(host, 'Remove configuration.bin').click();
  flush();
  expect(host.querySelector('.files .tree')!.textContent).not.toContain('configuration.bin');
});

test('a partial configuration read keeps the pages read', async () => {
  const test = createTestCameraModel({
    session: () => ({
      configurationGeometry: async () => ({ capacity: 10, pageBytes: 4 }),
      readConfiguration: async (offset, length) => {
        if (offset) throw Error('Camera stopped answering');
        return new Uint8Array(length);
      },
    }),
  });
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  button(host, 'Dump configuration').click();
  await vi.waitFor(() => {
    flush();
    expect(host.textContent).toContain('Camera stopped answering');
  });
  expect(host.querySelector('.files .tree')!.textContent).toContain('configuration.bin4 B');
});

test('Contribute lists the collected files and keeps Upload in a fixed footer', async () => {
  const test = createTestCameraModel({
    session: () => ({
      configurationGeometry: async () => ({ capacity: 4, pageBytes: 4 }),
      readConfiguration: async (_offset, length) => new Uint8Array(length),
    }),
  });
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
    this.open = true;
  };
  button(host, 'Dump configuration').click();
  await vi.waitFor(() => {
    flush();
    expect(button(host, 'Contribute').disabled).toBe(false);
  });
  button(host, 'Contribute').click();
  flush();
  const dialog = host.querySelector<HTMLDialogElement>('dialog.contribute')!;
  expect(dialog.open).toBe(true);
  expect(dialog.querySelector('.tree')!.textContent).toContain('configuration.bin');
  expect(dialog.querySelector('.tree button[aria-label^="Download"]')).toBeNull();
  expect(dialog.textContent).toContain('Message (optional)');
  expect(
    [...dialog.querySelectorAll('.cd-buttons button')].map((node) => node.textContent),
  ).toEqual(['Cancel', 'Upload']);
});

test('the Write tool shows the debug flag and disables debug mode when it applies', () => {
  const test = createTestCameraModel({});
  test.add(testCamera('X100V', { debugMode: undefined }));
  const host = mount(test.model);
  flush();
  const disable = () => host.querySelector<HTMLButtonElement>('.wb-tool.wb-write button')!;
  expect(host.querySelector('.wb-tool.wb-write .wb-writes')!.textContent).toBe(
    'Changes a camera setting',
  );
  expect(host.querySelector('.wb-debug-state')!.textContent).toBe('Debug mode: Reading…');
  test.update('X100V', (camera) => ({ ...camera, debugMode: 'off' }));
  flush();
  expect(host.querySelector('.wb-debug-state')!.textContent).toBe('Debug mode: Off');
  expect(disable().disabled).toBe(true);
  test.update('X100V', (camera) => ({ ...camera, debugMode: 'on', transport: 'remote' }));
  flush();
  expect(disable().disabled).toBe(true);
  test.update('X100V', (camera) => ({ ...camera, transport: 'usb' }));
  flush();
  disable().click();
  flush();
  expect(host.textContent).toContain('Turns the Debug Mode setting off');
});

test('a camera known to be in service mode gets a banner, and only then', () => {
  const test = createTestCameraModel({});
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  expect(host.querySelector('.wb-service')).toBeNull();
  expect(host.querySelector('.wb-cam-meta')!.textContent).not.toContain('Service mode');
  test.update('X100V', (camera) => ({ ...camera, serviceMode: true }));
  flush();
  expect(host.querySelector('.wb-service')!.textContent).toContain('restores factory settings');
  expect(host.querySelector('.wb-cam-meta')!.textContent).toContain('Service mode');
  test.update('X100V', (camera) => ({ ...camera, present: false }));
  flush();
  expect(host.querySelector('.wb-service')).toBeNull();
});

test('a remote tool waiting for the owner shows the spoken countdown', async () => {
  vi.useFakeTimers();
  const speak = vi.fn();
  vi.stubGlobal('speechSynthesis', { speak, cancel: vi.fn() });
  vi.stubGlobal(
    'SpeechSynthesisUtterance',
    class {
      constructor(public text: string) {}
    },
  );
  const test = createTestCameraModel({});
  test.add(testCamera('X100V'));
  const host = mount(test.model);
  flush();
  const done = test.countdown('X100V', 'Press the shutter');
  flush();
  expect(host.querySelector('.wb-countdown')!.textContent).toContain('Press the shutter');
  await vi.advanceTimersByTimeAsync(11000);
  await done;
  flush();
  expect(host.querySelector('.wb-countdown')).toBeNull();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
