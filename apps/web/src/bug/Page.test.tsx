import { Blob as NodeBlob } from 'node:buffer';
import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
import BugReport from './Page';
import { base64url } from './report-link';
import { tar, link, hash, reportTar } from './fixtures.test-helper';
import { listedReport } from './report-link';
import type { UploadReport } from './upload';
import type { ReportConnection } from './usb';
vi.mock('../dev-tools/Page', () => ({
  default: () => <p>Dev Tools connected</p>,
}));
const receipt = {
  id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  shareUrl: 'https://share.fujimod.com/contribution/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  issueURL: 'https://github.com/justjake/fujimod/issues/123',
};
function mount(props: Parameters<typeof BugReport>[0]) {
  vi.stubGlobal('Blob', NodeBlob);
  const host = document.createElement('div');
  document.body.append(host);
  const dispose = render(() => <BugReport {...props} />, host);
  return {
    host,
    dispose: () => {
      dispose();
      host.remove();
      vi.unstubAllGlobals();
    },
  };
}
function button(host: HTMLElement, name: string) {
  return [...host.querySelectorAll('button')].find((n) => n.textContent === name)!;
}
if (!HTMLDialogElement.prototype.showModal)
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.open = true;
  };
if (!HTMLDialogElement.prototype.close)
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
    this.open = false;
  };
const dialog = (host: HTMLElement) => host.querySelector<HTMLDialogElement>('dialog.contribute')!;
/** Opens the Contribute dialog and presses Upload, with an optional message. */
function contribute(host: HTMLElement, message?: string) {
  if (!dialog(host).open) {
    button(host, 'Contribute').click();
    flush();
  }
  if (message !== undefined) {
    const textarea = dialog(host).querySelector('textarea')!;
    textarea.value = message;
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    flush();
  }
  button(dialog(host), 'Upload').click();
}
test('two acquisition paths remain available; upload is disabled until an archive is read', async () => {
  const upload = vi.fn();
  const agent = vi
    .spyOn(navigator, 'userAgent', 'get')
    .mockReturnValue('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)');
  const { host, dispose } = mount({ hash: hash(), supported: false, upload });
  try {
    await vi.waitFor(() => {
      flush();
      expect(host.textContent).toContain('Choose from SD card');
    });
    expect(host.textContent).toContain('/FUJIMOD/REPORTS/R1234567.TAR');
    const usb = host.querySelector('[aria-label="USB connection"]')!;
    expect(usb.classList.contains('bug-usb-unavailable')).toBe(true);
    expect(usb.firstElementChild?.textContent).toBe('WebUSB unavailable');
    expect(host.querySelector('.bug-platform')?.textContent).toContain('iPhone · Files');
    expect(host.querySelector('h1')).toBeNull();
    expect(host.textContent).not.toContain('Nothing is uploaded automatically.');
    expect(host.textContent).toContain(
      'Your camera data is private and remains on your device until you press upload.',
    );
    expect(host.textContent).toContain('Chrome or Edge');
    expect(host.textContent).toContain('ready to upload your error report over USB');
    expect(button(host, 'Contribute').disabled).toBe(true);
    expect(button(host, 'Download all').disabled).toBe(true);
    expect(host.querySelector('input[type=checkbox]')).toBeNull();
    expect(upload).not.toHaveBeenCalled();
  } finally {
    dispose();
    agent.mockRestore();
  }
});
test('inline report lists real entries locally and uploads only on explicit action', async () => {
  const choose = vi.fn(),
    upload = vi.fn<UploadReport>(async () => receipt),
    remove = vi.fn();
  const { host, dispose } = mount({
    hash: hash({ ...link, archive: base64url(tar) }),
    supported: false,
    upload,
    connection: { choose, remove, dispose: vi.fn() } as unknown as ReportConnection,
  });
  try {
    await vi.waitFor(() => {
      flush();
      expect(button(host, 'Contribute').disabled).toBe(false);
    });
    expect(host.querySelectorAll('.bug-contents > .tree .r').length).toBeGreaterThan(0);
    expect(choose).not.toHaveBeenCalled();
    expect(upload).not.toHaveBeenCalled();
    button(host, 'Contribute').click();
    flush();
    expect(dialog(host).textContent).toContain('Your message will be posted publicly on GitHub.');
    expect(dialog(host).querySelector('input[type=checkbox]')).toBeNull();
    contribute(host, 'Frozen menu');
    await vi.waitFor(() => {
      flush();
      expect(dialog(host).textContent).toContain('View GitHub issue');
    });
    expect(upload.mock.calls[0]?.[2]).toBe('Frozen menu');
    expect(remove).not.toHaveBeenCalled();
  } finally {
    dispose();
  }
});
test('failed upload retains USB report; retry reuses ID and deletes only after durable acknowledgment', async () => {
  const remove = vi.fn(async () => {}),
    handOff = vi.fn(async () => {});
  const upload = vi.fn().mockRejectedValueOnce(Error('Offline')).mockResolvedValue(receipt);
  const connection = {
    initialize: async () => {},
    choose: async () => ({ name: 'Camera', interfaces: [{ id: 0, label: 'USB' }] }),
    read: async () => new Blob([tar]),
    remove,
    handOff,
    dispose: vi.fn(),
  } as unknown as ReportConnection;
  const { host, dispose } = mount({ hash: hash(), supported: true, connection, upload });
  try {
    await vi.waitFor(() => {
      flush();
      expect(button(host, 'Connect camera').disabled).toBe(false);
    });
    button(host, 'Connect camera').click();
    await vi.waitFor(() => {
      flush();
      expect(host.textContent).toContain('Report retrieved from camera.');
    });
    button(host, 'Contribute').click();
    flush();
    expect((dialog(host).querySelector('input[type=checkbox]') as HTMLInputElement).checked).toBe(
      true,
    );
    expect(remove).not.toHaveBeenCalled();
    contribute(host);
    await vi.waitFor(() => {
      flush();
      expect(dialog(host).querySelector('.cd-status')!.textContent).toContain('Offline');
    });
    expect(remove).not.toHaveBeenCalled();
    contribute(host);
    await vi.waitFor(() => {
      flush();
      expect(remove).toHaveBeenCalledOnce();
    });
    expect(upload.mock.calls[0][3]).toBe(upload.mock.calls[1][3]);
    button(dialog(host), 'Close').click();
    flush();
    button(host, 'Open Dev Tools').click();
    await vi.waitFor(() => {
      flush();
      expect(host.textContent).toContain('Dev Tools connected');
    });
    expect(handOff).toHaveBeenCalledOnce();
  } finally {
    dispose();
  }
});
test("USB reports show their screen and time, list the camera's others and offer the next", async () => {
  if (!URL.createObjectURL) URL.createObjectURL = () => 'blob:screen';
  if (!URL.revokeObjectURL) URL.revokeObjectURL = () => {};
  const first = reportTar({ 'screen.bmp': Uint8Array.of(0x42, 0x4d, 1) }, '2026-10-27T14:04:05');
  const qr = { ...link, bytes: first.length };
  const second = reportTar({ 'state.json': Uint8Array.of(123, 125) }, '2026-10-26T09:30:00');
  const listing = [
    { name: 'R1234567.TAR', bytes: first.length },
    { name: 'R00000AB.TAR', bytes: second.length },
    { name: 'R00000AC.TAR', bytes: second.length },
  ];
  const readListed = vi.fn(async (entry: { name: string }) => listedReport(second, qr, entry.name));
  const upload = vi.fn<UploadReport>(async () => receipt);
  const connection = {
    initialize: async () => {},
    choose: async () => ({ name: 'Camera', interfaces: [{ id: 0, label: 'USB' }] }),
    read: async () => new Blob([first]),
    list: async () => listing,
    readListed,
    remove: async () => {},
    handOff: async () => {},
    dispose: vi.fn(),
  } as unknown as ReportConnection;
  const { host, dispose } = mount({ hash: hash(qr), supported: true, connection, upload });
  try {
    await vi.waitFor(() => {
      flush();
      expect(button(host, 'Connect camera').disabled).toBe(false);
    });
    button(host, 'Connect camera').click();
    await vi.waitFor(() => {
      flush();
      expect(host.querySelector('.bug-others')).not.toBeNull();
    });
    expect(host.querySelector<HTMLImageElement>('img.bug-screen')!.alt).toBe(
      'Camera screen when the report was captured',
    );
    const when = new Date(2026, 9, 27, 14, 4, 5).toLocaleString(undefined, {
      dateStyle: 'medium',
      timeStyle: 'short',
    });
    expect(host.querySelector('.bug-contents')!.textContent).toContain(when);
    const rows = [...host.querySelectorAll('.bug-others li')];
    expect(rows.map((row) => row.querySelector('.bug-file')!.textContent)).toEqual(
      listing.map((entry) => entry.name),
    );
    expect(rows[0].textContent).toContain('Showing');
    contribute(host);
    await vi.waitFor(() => {
      flush();
      expect(button(dialog(host), 'Next report')).toBeTruthy();
    });
    button(dialog(host), 'Next report').click();
    await vi.waitFor(() => {
      flush();
      expect(dialog(host).open).toBe(true);
      expect(button(dialog(host), 'Upload')).toBeTruthy();
    });
    expect(readListed.mock.calls[0]![0]).toMatchObject({ name: 'R00000AB.TAR' });
    expect(host.querySelector('img.bug-screen')).toBeNull();
    expect(host.querySelector('.bug-others li')!.textContent).toContain('Contributed');
    contribute(host);
    await vi.waitFor(() => {
      flush();
      expect(upload).toHaveBeenCalledTimes(2);
      expect(button(dialog(host), 'Next report')).toBeTruthy();
    });
    expect(upload.mock.calls[1]![1].path).toBe('A:\\FUJIMOD\\REPORTS\\R00000AB.TAR');
  } finally {
    dispose();
  }
});
