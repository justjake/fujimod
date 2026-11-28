/**
 * Screenshot the desktop launcher at 2x with a stubbed native bridge.
 *
 * Run the app's dev server (`pnpm dev`) or set FUJIMOD_GUI_URL to it, then run
 * `pnpm screenshot:launcher`. The image is written under build/emulator-gui/.
 */
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';
import type { DesktopBridge } from './launch';

const url = process.env.FUJIMOD_GUI_URL ?? 'http://localhost:1420';
const output = new URL('../../../build/emulator-gui/', import.meta.url);
await mkdir(output, { recursive: true });

const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 720 },
    deviceScaleFactor: 2,
  });
  await context.addInitScript(() => {
    const bridge = {
      chooseUpdate: async () => undefined,
      launch: () => Promise.reject(Error('The screenshot does not open a camera.')),
      launcherCameras: async () => [
        { firmware: 'x70-110', body: 'x70', model: 'X70', version: '1.10' },
        { firmware: 'x100vi-131', body: 'x100vi', model: 'X100VI', version: '1.31' },
        { firmware: 'xt5-431', body: 'xt5', model: 'X-T5', version: '4.31' },
      ],
      chooseSdCard: async () => '/Volumes/FUJIMOD',
      openCamera: () => Promise.reject(Error('The screenshot does not open a camera.')),
      connect: () => Promise.reject(Error('The screenshot does not open a camera.')),
      launchConnection: async () => undefined,
      connection: async () => undefined,
      close: async () => {},
    } satisfies DesktopBridge;
    (window as Window & { fujimodDesktop?: DesktopBridge }).fujimodDesktop = bridge;
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.locator('.launcher .boot-choice').waitFor();
  await page.waitForFunction(
    () => document.querySelectorAll('.launcher select option').length === 3,
  );
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  assert.deepEqual(errors, []);
  const screenshot = new URL('launcher@2x.png', output);
  await page.screenshot({ path: screenshot.pathname });
  console.log(`launcher screenshot: ${screenshot.pathname}`);
} finally {
  await browser.close();
}
