import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { ResolvedSelection } from 'fujimod';
import { createTestCameraModel, testCamera } from '../test-camera-model';
import { fileDevice } from '../../device-files.test-helper';
import { reportTar } from '../../bug/fixtures.test-helper';
import type { UploadReport } from '../../bug/upload';

const selection = {
  firmware: { id: 'x70-110', version: '1.10' },
  bulk: { product: 'X70', internal_version: '1.39', scopes: [] },
} as unknown as ResolvedSelection;
vi.mock('fujimod', () => ({ catalog: { selections: () => [selection] } }));
const { BugReports } = await import('./BugReports');

if (!HTMLDialogElement.prototype.showModal)
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.open = true;
  };
if (!HTMLDialogElement.prototype.close)
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
    this.open = false;
  };
let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  document.body.innerHTML = '';
});

/** 24-byte listing records: little-endian size, then the NUL-padded name. */
function records(entries: [string, number][]) {
  const bytes = new Uint8Array(entries.length * 24);
  entries.forEach(([name, size], i) => {
    new DataView(bytes.buffer).setUint32(i * 24, size, true);
    bytes.set(new TextEncoder().encode(name), i * 24 + 4);
  });
  return bytes;
}

test('a connected camera lists its reports, and Report files one issue per report then deletes them', async () => {
  const tar = reportTar({ 'screen.bmp': new Uint8Array(64) }, '2026-10-26T12:00:00Z');
  const device = fileDevice(
    tar,
    0xaaaaaaaa,
    2,
    records([
      ['R00000AB.TAR', tar.length],
      ['R00000CD.TAR', tar.length],
    ]),
  );
  const sessions: string[] = [];
  const test = createTestCameraModel({
    session: () => ({
      async memory(firmware) {
        sessions.push(firmware);
        return { ...device.memory, overwritten: null, close: async () => {} };
      },
    }),
  });
  test.add(testCamera('x70', { identity: { product: 'X70', internalVersion: '1.39' } }));
  flush();
  const upload = vi.fn<UploadReport>(async (_archive, link, _message, id) => ({
    id,
    shareUrl: `https://share.fujimod.com/contribution/${link.path.slice(-12, -4)}`,
    issueURL: 'https://github.com/justjake/fujimod/issues/1',
  }));
  const files: string[] = [];
  const host = document.createElement('div');
  document.body.append(host);
  cleanup = render(
    () => (
      <BugReports
        camera={test.model.cameras()[0]}
        model={test.model}
        onFile={(name) => files.push(name)}
        upload={upload}
      />
    ),
    host,
  );
  await vi.waitFor(() => {
    flush();
    expect(host.querySelector('.br-list')?.textContent).toContain('R00000CD.TAR');
  });
  expect(host.querySelector('.br-wiggle')).not.toBeNull();
  const report = [...host.querySelectorAll('button')].find(
    (node) => node.textContent === 'Report 2 bugs',
  )!;
  report.click();
  await vi.waitFor(() =>
    expect(files).toEqual(['bug-report-r00000ab.tar', 'bug-report-r00000cd.tar']),
  );
  flush();
  const dialog = host.querySelector('dialog')!;
  [...dialog.querySelectorAll('button')].find((node) => node.textContent === 'Upload')!.click();
  await vi.waitFor(() => expect(device.commands.filter((op) => op === 7)).toHaveLength(2));
  expect(upload.mock.calls.map((call) => call[1].path)).toEqual([
    'A:\\FUJIMOD\\REPORTS\\R00000AB.TAR',
    'A:\\FUJIMOD\\REPORTS\\R00000CD.TAR',
  ]);
  expect(new Set(upload.mock.calls.map((call) => call[3])).size).toBe(2);
  expect(sessions.every((firmware) => firmware === 'x70-110')).toBe(true);
});
