import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import { ContributeDialog, type ContributeReceipt } from './ContributeDialog';
import { sharingUrl } from '../sharing-server';

const receipt: ContributeReceipt = {
  id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  shareUrl: sharingUrl('/contribution/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'),
};
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});
function mount(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
    configurable: true,
    writable: true,
    value: function (this: HTMLDialogElement) {
      this.open = true;
    },
  });
  const upload = vi.fn(async () => receipt);
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => <ContributeDialog open items={[]} upload={upload} onClose={() => {}} />,
    host,
  );
  flush();
  return { host, upload };
}
function click(host: HTMLElement, label: string) {
  [...host.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === label)!
    .click();
}
test('successful contribution copies the read-only share URL and announces completion', async () => {
  const copy = vi.fn(async (_text: string) => {});
  const { host, upload } = mount(copy);
  click(host, 'Upload');
  await vi.waitFor(() => {
    flush();
    expect(host.textContent).toContain('Share URL copied');
  });
  expect(copy).toHaveBeenCalledWith(receipt.shareUrl);
  expect(upload).toHaveBeenCalledTimes(1);
  expect(host.querySelector<HTMLAnchorElement>('a')!.href).toBe(receipt.shareUrl);
  expect(host.querySelector('[role=status]')!.textContent).toContain('Share URL copied');
});
test('clipboard denial keeps the successful upload and offers a copy retry without uploading again', async () => {
  const copy = vi
    .fn<(text: string) => Promise<void>>()
    .mockRejectedValueOnce(new DOMException('Denied', 'NotAllowedError'))
    .mockResolvedValue(undefined);
  const { host, upload } = mount(copy);
  click(host, 'Upload');
  await vi.waitFor(() => {
    flush();
    expect(host.textContent).toContain('Copy URL');
  });
  expect(host.textContent).toContain('Uploaded');
  expect(host.textContent).not.toContain('Share URL copied');
  click(host, 'Copy URL');
  await vi.waitFor(() => {
    flush();
    expect(host.textContent).toContain('Share URL copied');
  });
  expect(upload).toHaveBeenCalledTimes(1);
  expect(copy).toHaveBeenCalledTimes(2);
});
