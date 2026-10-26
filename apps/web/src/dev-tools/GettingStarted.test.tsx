import { render } from '@solidjs/web';
import { createSignal, flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { ResolvedSelection } from 'fujimod';
import type { ToolSession } from './camera-model';
import { createTestCameraModel, testCamera } from './test-camera-model';
import type { PathResults } from './GettingStarted';

const selection = {
  firmware: { id: 'xm5-130', version: '1.30' },
  bulk: { product: 'X-M5', internal_version: '2.06', scopes: [] },
} as unknown as ResolvedSelection;
vi.mock('fujimod', () => ({ catalog: { selections: () => [selection] } }));
const { GettingStarted } = await import('./GettingStarted');

let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  document.body.innerHTML = '';
  localStorage.clear();
});
const button = (host: HTMLElement, name: string) =>
  [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === name);

function mount(session: Partial<ToolSession>, serial = 'SERIAL1') {
  const test = createTestCameraModel({ session: () => session });
  test.add(
    testCamera('m5', {
      serviceMode: true,
      identity: { product: 'X-M5', internalVersion: '2.06', serial },
    }),
  );
  flush();
  const [results, setResults] = createSignal<PathResults>({});
  const enter = vi.fn(async (_firmware: string, progress: (line: string) => void) =>
    progress('Debug mode enabled'),
  );
  const files: { name: string; blob: Blob; meta?: Record<string, unknown> }[] = [];
  const host = document.createElement('div');
  document.body.append(host);
  cleanup = render(
    () => (
      <GettingStarted
        camera={test.model.cameras()[0]!}
        model={test.model}
        results={results()}
        onResults={(update) => setResults(update)}
        onFile={(name, blob, meta) => files.push({ name, blob, meta })}
        debug={{ enter, leave: vi.fn() }}
        onWizard={() => {}}
      />
    ),
    host,
  );
  flush();
  return { host, enter, results, test, files };
}
const prepared = { guardBefore: 0, debugBefore: 0, progress: [] };

test('debug mode stays locked until the flash backup is skipped, and the serial is remembered', async () => {
  const prepareServiceSession = vi.fn(async () => prepared);
  const p = mount({ prepareServiceSession });
  expect(button(p.host, 'Enable debug mode')!.disabled).toBe(true);
  button(p.host, 'Skip')!.click();
  flush();
  expect(p.host.textContent).toContain('Skipped');
  const enable = button(p.host, 'Enable debug mode')!;
  expect(enable.disabled).toBe(false);
  enable.click();
  await vi.waitFor(() => expect(p.results().debugSaved).toBe(true));
  expect(p.enter).toHaveBeenCalledWith('xm5-130', expect.any(Function));
  cleanup?.();
  const again = mount({ prepareServiceSession });
  expect(button(again.host, 'Enable debug mode')!.disabled).toBe(false);
  cleanup?.();
  const other = mount({ prepareServiceSession }, 'SERIAL2');
  expect(button(other.host, 'Enable debug mode')!.disabled).toBe(true);
});

test('the flash backup prepares the session, assembles chunks by offset and adds flash.bin', async () => {
  const calls: string[] = [];
  const p = mount({
    async prepareServiceSession(firmware) {
      calls.push(`prepare ${firmware}`);
      return { guardBefore: 0, debugBefore: 1, progress: [] };
    },
    async backupFlash(firmware, chunk) {
      calls.push(`flash ${firmware}`);
      chunk({ offset: 4, data: new Uint8Array([5, 6, 7, 8]) });
      chunk({ offset: 0, data: new Uint8Array([1, 2, 3, 4]) });
      return {
        device: { kind: 'serial-nand', part: 'W35N01JW' },
        size: 8,
        sha256: 'ab',
      };
    },
  });
  button(p.host, 'Back up flash')!.click();
  await vi.waitFor(() => expect(p.results().backup).toBeTruthy());
  expect(calls).toEqual(['prepare xm5-130', 'flash xm5-130']);
  expect(p.results().prepared?.debugBefore).toBe(1);
  expect(p.files.map((file) => file.name)).toEqual(['flash.bin']);
  expect([...new Uint8Array(await p.files[0]!.blob.arrayBuffer())]).toEqual([
    1, 2, 3, 4, 5, 6, 7, 8,
  ]);
  expect(p.files[0]!.meta).toMatchObject({ firmware: 'xm5-130', flash: { size: 8 } });
  expect(button(p.host, 'Enable debug mode')!.disabled).toBe(false);
});

test('a flash backup with a gap is refused and unlocks nothing', async () => {
  const p = mount({
    prepareServiceSession: async () => prepared,
    async backupFlash(_firmware, chunk) {
      chunk({ offset: 4, data: new Uint8Array(4) });
      return { device: { kind: 'serial-nor', part: 'x' }, size: 8, sha256: '' };
    },
  });
  button(p.host, 'Back up flash')!.click();
  await vi.waitFor(() => {
    flush();
    expect(p.host.textContent).toContain('gap at 0x00000000');
  });
  expect(p.results().backup).toBeUndefined();
  expect(p.files).toEqual([]);
  expect(button(p.host, 'Enable debug mode')!.disabled).toBe(true);
});

test('Dump memory reports bytes read and adds each region as its own file', async () => {
  const statuses: string[] = [];
  const status = () => {
    flush();
    statuses.push(p.host.querySelector('[role="status"]')?.textContent ?? '');
  };
  const p = mount({
    prepareServiceSession: async () => prepared,
    async dumpState(_firmware, region, progress) {
      status();
      progress?.(1 << 20, 4 << 20);
      status();
      region({
        name: 'boot-rom',
        address: 0x80000000,
        size: 4,
        constant: true,
        sha256: 'cd',
        data: new Uint8Array(4),
      });
    },
  });
  button(p.host, 'Dump memory')!.click();
  await vi.waitFor(() => expect(p.results().state).toBeTruthy());
  expect(p.results().state!.regions.map((region) => region.name)).toEqual(['boot-rom']);
  expect(statuses).toEqual(['Dumping memory', 'Dumping memory · 1 MiB of 4 MiB']);
  expect(p.files.map((file) => file.name)).toEqual(['memory-boot-rom.bin']);
  expect(p.files[0]!.meta).toMatchObject({ address: 0x80000000, constant: true, sha256: 'cd' });
});

test('a failed step shows the camera’s refusal', async () => {
  const p = mount({
    prepareServiceSession: async () => {
      throw Error('Service mode did not verify');
    },
  });
  button(p.host, 'Dump memory')!.click();
  await vi.waitFor(() => {
    flush();
    expect(p.host.textContent).toContain('Service mode did not verify');
  });
});
