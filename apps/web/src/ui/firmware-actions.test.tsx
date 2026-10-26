import { test, expect, beforeAll, afterEach } from 'vite-plus/test';
import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { flags } from '@fujimod/emulator-ui/flags';
import { firmwareActions, createFirmwareChoice, FirmwareChoice } from './firmware-actions';
let dispose: (() => void) | undefined;
beforeAll(() => {
  HTMLDialogElement.prototype.close = function () {
    this.open = false;
    this.dispatchEvent(new Event('close'));
  };
  HTMLDialogElement.prototype.showModal = function () {
    this.open = true;
  };
});
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
test('release defaults launch directly without opening a dialog', async () => {
  expect(flags).toEqual({ allowFpUpdateDownload: false, allowSdCardBootDownload: false });
  const choice = createFirmwareChoice();
  expect(await choice.choose(firmwareActions(flags, { fpUpdate: true, sdCard: true }), 'X70')).toBe(
    'launch',
  );
  expect(document.querySelector('dialog')).toBeNull();
});
test('independent download gates require a release output', () => {
  for (const fp of [false, true])
    for (const sd of [false, true]) {
      const gates = { allowFpUpdateDownload: fp, allowSdCardBootDownload: sd };
      expect(firmwareActions(gates, { fpUpdate: true, sdCard: true })).toEqual([
        'launch',
        ...(fp ? ['fp-update'] : []),
        ...(sd ? ['sd-card'] : []),
      ]);
      expect(firmwareActions(gates, { fpUpdate: false, sdCard: false })).toEqual(['launch']);
    }
});
test('every available firmware action resolves its native dialog selection', async () => {
  const choice = createFirmwareChoice();
  dispose = render(() => <FirmwareChoice choice={choice} />, document.body);
  for (const action of ['launch', 'sd-card', 'fp-update'] as const) {
    const chosen = choice.choose(['launch', 'sd-card', 'fp-update'], 'X70 · 1.10');
    flush();
    const dialog = document.querySelector('dialog')!;
    expect(dialog.open).toBe(true);
    expect([...dialog.querySelectorAll('button')].map((button) => button.textContent)).toEqual([
      'Launch camera',
      'Download SD card files',
      'Download FP update',
      'Cancel',
    ]);
    dialog.returnValue = action;
    dialog.close();
    flush();
    expect(await chosen).toBe(action);
    expect(dialog.open).toBe(false);
  }
});
test('a new firmware selection cancels the previous pending choice', async () => {
  const choice = createFirmwareChoice();
  dispose = render(() => <FirmwareChoice choice={choice} />, document.body);
  const chosen = choice.choose(['launch', 'sd-card'], 'X70');
  flush();
  choice.cancel();
  flush();
  expect(await chosen).toBeUndefined();
  expect(document.querySelector('dialog')!.open).toBe(false);
});
