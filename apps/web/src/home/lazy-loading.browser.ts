/** Verify deferred emulator loading with real downloads held at their network boundaries. */
import { chromium, webkit, type Page } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
const dat = process.env.FUJIMOD_TEST_DAT;
if (!base || !dat) throw Error('Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/lazy-loading/', import.meta.url);
await mkdir(output, { recursive: true });
function gate() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}
async function waitForGate(promise: Promise<void>, label: string) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(Error(`Timed out waiting for ${label}`)), 60000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function loading(page: Page) {
  const bar = page.locator('#camera-loading progress:visible').first();
  assert.equal(await bar.isVisible(), true);
  const value = await bar.getAttribute('value');
  assert.equal(value, null, 'Waiting for headers has no fabricated percentage');
  const bounds = await bar.boundingBox();
  assert.ok(bounds && bounds.width > 100 && bounds.height > 0);
}
for (const type of [chromium, webkit]) {
  const browser = await type.launch();
  const moduleReady = gate(),
    modulePermit = gate(),
    runtimeReady = gate(),
    runtimePermit = gate();
  try {
    const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
    await context.routeWebSocket('**/*', () => {});
    await context.route('**/src/engine/index.ts*', async (route) => {
      moduleReady.release();
      await modulePermit.promise;
      await route.continue();
    });
    await context.route('**/camera.wasm', async (route) => {
      runtimeReady.release();
      await runtimePermit.promise;
      await route.continue();
    });
    const page = await context.newPage(),
      requests: string[] = [],
      workers: string[] = [],
      errors: string[] = [];
    page.setDefaultTimeout(240000);
    page.on('worker', (worker) => workers.push(worker.url()));
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => {
      if (
        /\/src\/engine\/(index|wasm-engine)\.ts|\/runtime\/(?!catalog\.json)|default-scene|wasm-worker/.test(
          request.url(),
        )
      )
        requests.push(request.url());
    });
    await page.goto(base, { waitUntil: 'networkidle' });
    await page.waitForFunction(
      () => performance.getEntriesByName('camera-catalog-ready').length > 0,
    );
    assert.deepEqual(
      requests,
      [],
      'The idle page must not load emulator modules or runtime assets',
    );
    assert.equal(workers.length, 0, 'The idle page must not create an emulator worker');
    assert.equal(await page.locator('#boot-progress').isVisible(), false);
    console.log(`${type.name()}: idle page has no emulator downloads or worker`);
    const chooser = page.waitForEvent('filechooser');
    await page.locator('#firmware-prompt').click();
    await (await chooser).setFiles(dat);
    await waitForGate(moduleReady.promise, 'the lazy engine import');
    await loading(page);
    assert.equal(workers.length, 0);
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    await page.screenshot({ path: new URL(`${type.name()}-module.png`, output).pathname });
    modulePermit.release();
    await waitForGate(runtimeReady.promise, 'the runtime download');
    assert.equal(
      await page
        .locator('.loading-assets li[data-asset$="camera.wasm"]')
        .getAttribute('data-state'),
      'waiting',
    );
    await loading(page);
    assert.equal(workers.filter((url) => url.includes('wasm-worker')).length, 1);
    await page.screenshot({ path: new URL(`${type.name()}-download.png`, output).pathname });
    runtimePermit.release();
    await page.waitForFunction(() => document.querySelector('#live-label')?.textContent === 'LIVE');
    assert.equal(
      await page
        .locator('.loading-assets li[data-asset$="camera.wasm"]')
        .getAttribute('data-state'),
      'complete',
    );
    assert.equal(await page.locator('#boot-progress').isVisible(), false);
    assert.equal(await page.locator('#camera-loading').isVisible(), false);
    assert.equal(
      await page.locator('#lcd-source').evaluate((canvas: HTMLCanvasElement) =>
        canvas
          .getContext('2d')!
          .getImageData(0, 0, canvas.width, canvas.height)
          .data.some((value) => value > 0),
      ),
      true,
    );
    assert.deepEqual(errors, []);
    await page.screenshot({ path: new URL(`${type.name()}-ready.png`, output).pathname });
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify(
        { requests, workers, errors, phases: ['idle', 'module', 'download', 'ready'] },
        null,
        2,
      ),
    );
    console.log(
      `${type.name()}: lazy import, waiting download, completed assets and live LCD passed`,
    );
  } finally {
    modulePermit.release();
    runtimePermit.release();
    await browser.close();
  }
}
