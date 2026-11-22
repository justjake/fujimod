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
import { zipBlob } from '@fujimod/emulator-ui/archive';
import { uploadReportArchive } from '../files/contribute';
import { sharingUrl } from '../sharing-server';
vi.mock('../files/contribute', () => ({ uploadReportArchive: vi.fn(async () => {}) }));
vi.mock('@fujimod/emulator-ui/archive', () => ({
  zipBlob: vi.fn(async () => new NodeBlob(['zip'])),
}));
vi.mock('../dev-tools/Page', () => ({
  default: () => <p>Dev Tools connected</p>,
}));
const receipt = {
  id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  shareUrl: sharingUrl('/contribution/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'),
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
  const first = reportTar(
    { 'screen/video.yuv': Uint8Array.of(16, 16, 128, 128) },
    '2026-10-27T14:04:05',
    { screen: { video: 'screen/video.yuv', width: 2, height: 1 } },
  );
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

test('without a QR link, users can select arbitrary files and submit a report with optional contact', async () => {
  const fetcher = vi.fn<typeof fetch>(async (_url, request) =>
    Response.json({
      id: new Headers(request?.headers).get('Idempotency-Key'),
      issueURL: 'https://github.com/justjake/fujimod/issues/123',
    }),
  );
  vi.stubGlobal('fetch', fetcher);
  const { host, dispose } = mount({ hash: '', supported: false });
  try {
    flush();
    const select = host.querySelector<HTMLSelectElement>('select[name="contact-type"]')!;
    expect([...select.options].map((option) => option.text)).toEqual([
      'None',
      'Instagram',
      'Twitter',
      'GitHub',
      'Email',
    ]);
    expect(host.querySelector('input[name="contact-value"]')).toBeNull();
    const description = host.querySelector<HTMLTextAreaElement>('textarea[name=description]')!;
    description.value = 'Screen freezes';
    select.value = 'GitHub';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    flush();
    expect(description.value).toBe('Screen freezes');
    host.querySelector<HTMLInputElement>('input[name="contact-value"]')!.value = '@reporter';
    const shareContact = host.querySelector<HTMLInputElement>('input[name="share-contact"]')!;
    const shareFiles = host.querySelector<HTMLInputElement>('input[name="share-files"]')!;
    expect(shareContact.checked).toBe(false);
    expect(shareFiles.checked).toBe(false);
    shareFiles.click();
    button(host, 'Linux').click();
    flush();
    expect(host.textContent).toContain('File manager → Devices');
    host.querySelector<HTMLInputElement>('input[name="name"]')!.value = 'Reporter';
    const picker = host.querySelector<HTMLInputElement>('input[type=file]')!;
    expect(picker.multiple).toBe(true);
    expect(picker.hasAttribute('accept')).toBe(false);
    Object.defineProperty(picker, 'files', {
      value: [new File(['log'], 'camera.log'), new File(['video'], 'sample.mp4')],
    });
    picker.dispatchEvent(new Event('change', { bubbles: true }));
    flush();
    expect(host.textContent).toContain('sample.mp4');
    const directory = host.querySelector<HTMLInputElement>('input[webkitdirectory]')!;
    const file = (path: string) =>
      Object.defineProperty(new File(['data'], path.split('/').at(-1)!), 'webkitRelativePath', {
        value: path,
      });
    Object.defineProperty(directory, 'files', {
      value: [
        file('FUJI/FUJIMOD/REPORTS/R1234567.TAR'),
        file('FUJI/FUJIMOD/AUTOSTRT.TXT'),
        file('FUJI/BACKUP/X70_001.IMG'),
        file('FUJI/ffboot.bin'),
        file('FUJI/DCIM/100_FUJI/DSCF0123.JPG'),
        file('FUJI/other.txt'),
      ],
    });
    directory.dispatchEvent(new Event('change', { bubbles: true }));
    flush();
    expect(host.textContent).not.toMatch(/DSCF0123|other.txt/);
    const check = (text: string) =>
      [...host.querySelectorAll<HTMLLabelElement>('ul.bug-attachments label')]
        .find((label) => label.textContent?.includes(text))!
        .querySelector('input')!;
    expect(check('X70_001.IMG').checked).toBe(true);
    button(host, 'Uncheck all').click();
    flush();
    expect(host.querySelectorAll('ul.bug-attachments input:checked')).toHaveLength(0);
    button(host, 'Check all').click();
    flush();
    expect(check('X70_001.IMG').checked).toBe(true);
    check('sample.mp4').click();
    flush();
    expect(fetcher).not.toHaveBeenCalled();
    button(host, 'Submit report').click();
    await vi.waitFor(() => {
      flush();
      expect(host.textContent).toContain('Report saved.');
    });
    const body = fetcher.mock.calls[0]?.[1]?.body as FormData;
    expect(body.get('name')).toBe('Reporter');
    expect(body.get('contact-type')).toBe('GitHub');
    expect(body.get('contact-value')).toBe('@reporter');
    expect(body.get('share-contact')).toBeNull();
    expect(body.get('share-files')).toBe('on');
    expect(body.get('archive')).toBe('zip');
    expect(vi.mocked(zipBlob).mock.lastCall?.[0].map((entry) => entry.path)).toEqual([
      'Attachments/1/camera.log',
      'BACKUP/X70_001.IMG',
      'ffboot.bin',
      'FUJIMOD/AUTOSTRT.TXT',
      'FUJIMOD/REPORTS/R1234567.TAR',
    ]);
    expect(uploadReportArchive).toHaveBeenCalledOnce();
  } finally {
    dispose();
  }
});
