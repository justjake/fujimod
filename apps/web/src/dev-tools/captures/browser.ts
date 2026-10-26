/** Real stock execution: a simulated X70 runs its stock bulk query path for a state capture. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import type { Catalog } from '../../catalog';
import type { CameraStore } from '../../cameras/store';
import type { CameraModel } from '../camera-model';
declare global {
  interface Window {
    __fujimodCameraModel?: CameraModel;
  }
}

const firmwareId = process.env.FUJIMOD_TEST_FIRMWARE || 'x70-110';
const url = new URL(
  'dev-tools/state-captures/',
  process.env.FUJIMOD_WEB_URL || 'http://127.0.0.1:5204/',
);
const output = new URL(`../../../build/state-captures/${firmwareId}/`, import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 1300, height: 900 },
  acceptDownloads: true,
});
const page = await context.newPage();
const errors: string[] = [];
page.on('pageerror', (error) => errors.push(error.message));
try {
  await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 90_000 });
  await page.waitForFunction(() => window.__fujimodCameraModel, undefined, { timeout: 90_000 });
  const savedId = await page.evaluate(async (id) => {
    const module = (await import(
      /* @vite-ignore */ new URL('/src/cameras/store.ts', location.href).href
    )) as { CameraStore: typeof CameraStore };
    const catalog = (await (await fetch('/runtime/catalog.json')).json()) as Catalog;
    const firmware = catalog.firmwares.find((entry) => entry.id === id);
    if (!firmware) throw Error(`No runtime for ${id}`);
    const response = await fetch(`/local/firmware/${id}.dat`);
    if (!response.ok) throw Error(`Firmware mount failed: ${id} (${response.status})`);
    const file = new File([await response.arrayBuffer()], 'camera.dat');
    return (await new module.CameraStore().create(file, firmware, false)).id;
  }, firmwareId);
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 90_000 });
  await page.waitForFunction(() => 'connectSimulated' in (window.__fujimodCameraModel ?? {}));
  await page.evaluate((id) => window.__fujimodCameraModel!.connectSimulated!(id), savedId);
  await page.waitForFunction(
    () => {
      const camera = window.__fujimodCameraModel?.cameras()[0];
      if (camera?.problem) throw Error(camera.problem);
      return camera?.identity?.internalVersion && !camera.session;
    },
    undefined,
    { timeout: 240_000 },
  );
  const row = page.locator('.cap-row', { hasText: 'Still shooting' });
  await row.getByRole('button', { name: 'Capture' }).click();
  await row.locator('.cap-count').waitFor({ timeout: 600_000 });
  await page.screenshot({ path: new URL('captured.png', output).pathname, fullPage: true });
  const capture = page.locator('.files .tree .r', { hasText: /^capture-still-shooting-/ }).first();
  await capture.waitFor();
  const name = (await capture.locator('.n').innerText()).trim();
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    capture.getByRole('button', { name: `Download ${name}` }).click(),
  ]);
  const path = new URL(name, output).pathname;
  await download.saveAs(path);
  const bytes = (await readFile(path)).length;
  assert.ok(bytes > 0, 'the capture file holds the captured bytes');
  assert.deepEqual(errors, []);
  console.log(`PASS ${firmwareId}: ${bytes} bytes captured through the stock bulk path`);
} catch (error) {
  await page.screenshot({ path: new URL('failure.png', output).pathname, fullPage: true });
  throw error;
} finally {
  await browser.close();
}
