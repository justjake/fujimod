/** Run against a development server with a configured local DAT. */
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';

const url = process.env.FUJIMOD_WEB_URL;
if (!url) throw Error('Set FUJIMOD_WEB_URL to the development homepage.');
if (new URL(url).searchParams.has('scenario')) throw Error('Use the ordinary homepage URL.');
const output = new URL('../../build/camera-library/', import.meta.url);
await mkdir(output, { recursive: true });

for (const type of [chromium, webkit].filter(
  (browser) =>
    !process.env.FUJIMOD_TEST_BROWSER || browser.name() === process.env.FUJIMOD_TEST_BROWSER,
)) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1440, height: 1080 } });
    // Keep concurrent development edits from navigating away during an IndexedDB write.
    await context.routeWebSocket('**/*', () => {});
    const page = await context.newPage();
    page.setDefaultTimeout(60000);
    const errors: string[] = [],
      automaticFirmware: string[] = [],
      engines: string[] = [];
    let seeding = false;
    page.on('pageerror', (error) => {
      errors.push(error.message);
      console.error(error.message);
    });
    page.on('request', (request) => {
      if (!seeding && new URL(request.url()).pathname.startsWith('/local/firmware'))
        automaticFirmware.push(request.url());
    });
    page.on('worker', (worker) => {
      if (worker.url().includes('wasm-worker')) engines.push(worker.url());
    });
    async function records() {
      return page.evaluate(async () => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        return new CameraStore().list();
      });
    }
    async function chooser(expected: import('./store').CameraRecord[]) {
      await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
      await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
      await page.waitForFunction(() =>
        document.querySelector<HTMLElement>('#viewer')?.style.getPropertyValue('--body-preview'),
      );
      assert.equal(await page.locator('.resume-camera').count(), expected.length);
      assert.equal(await page.locator('#camera-exit').count(), 0);
      assert.equal(await page.locator('#live-label').innerText(), '');
      assert.deepEqual(await records(), expected);
      assert.deepEqual(automaticFirmware, [], 'Loading the homepage must not select a DAT');
      assert.deepEqual(engines, [], 'Loading the homepage must not start an emulator');
      assert.deepEqual(errors, []);
    }
    await page.goto(url, { waitUntil: 'domcontentloaded' });
    console.log(`${type.name()}: opened development homepage`);
    // Ensure this exercises the development configuration that used to auto-boot.
    const mounted: import('playwright').APIResponse = await context.request.head(
      new URL('/local/firmware.dat', url).href,
    );
    assert.equal(mounted.status(), 200);
    assert.match(mounted.headers()['content-type'], /octet-stream/);
    await chooser([]);
    for (let refresh = 0; refresh < 3; refresh++) {
      await page.reload({ waitUntil: 'domcontentloaded' });
      await chooser([]);
    }
    console.log(`${type.name()}: empty library unchanged after three refreshes`);
    // Store real firmware without booting it: returning users can have unopened cameras.
    seeding = true;
    const saved = await page.evaluate(async () => {
      const { CameraStore }: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      const response = await fetch('/local/firmware.dat');
      if (!response.ok) throw Error('Local DAT unavailable.');
      const bytes = await response.arrayBuffer();
      const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))]
        .map((byte) => byte.toString(16).padStart(2, '0'))
        .join('');
      const catalog = await (await fetch('/runtime/catalog.json')).json();
      const firmware = catalog.firmwares.find((entry: { sha256: string }) => entry.sha256 === hash);
      if (!firmware) throw Error('Local DAT is missing from the catalog.');
      const store = new CameraStore(),
        file = new File([bytes], 'camera.dat');
      await store.create(file, firmware, false);
      await store.create(file, firmware, false);
      return store.list();
    });
    seeding = false;
    assert.equal(saved.length, 2);
    assert.notEqual(saved[0].id, saved[1].id);
    for (let refresh = 0; refresh < 3; refresh++) {
      await page.reload({ waitUntil: 'domcontentloaded' });
      await chooser(saved);
    }
    const hashes = await page.evaluate(async () => {
      const { CameraStore }: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      const store = new CameraStore();
      return Promise.all(
        (await store.list()).map(async (camera) => {
          const file = await store.firmware(camera.firmwareHash);
          const hash = [
            ...new Uint8Array(await crypto.subtle.digest('SHA-256', await file.arrayBuffer())),
          ]
            .map((byte) => byte.toString(16).padStart(2, '0'))
            .join('');
          return hash;
        }),
      );
    });
    assert.deepEqual(
      hashes,
      saved.map((camera) => camera.firmwareHash),
    );
    await page.screenshot({
      path: new URL(`startup-${type.name()}.png`, output).pathname,
      fullPage: true,
    });
    console.log(
      `${type.name()}: three empty-library and three saved-library refreshes; no creation or boot`,
    );
  } finally {
    await browser.close();
  }
}
