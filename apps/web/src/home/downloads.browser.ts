/** Verify transfer progress and stage labels during a real, bandwidth-limited cold boot. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const url = process.env.FUJIMOD_WEB_URL;
const dat = process.env.FUJIMOD_TEST_DAT;
if (!url || !dat) throw Error('Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/download-progress/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
try {
  const errors: string[] = [];
  if (!process.env.FUJIMOD_PREVIEW_ONLY) {
    const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
    await context.routeWebSocket('**/*', () => {});
    const page = await context.newPage();
    page.on('pageerror', (error) => errors.push(error.stack ?? error.message));
    page.on('console', (message) => {
      if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
    });
    await page.goto(url);
    await page.waitForFunction(
      () => performance.getEntriesByName('camera-catalog-ready').length > 0,
    );
    // Throttle actual served bytes, without replacing the runtime, firmware or worker.
    const cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    await cdp.send('Network.setBlockedURLs', { urls: ['wss://*', 'ws://*'] });
    await cdp.send('Network.emulateNetworkConditions', {
      offline: false,
      latency: 20,
      downloadThroughput: 400000,
      uploadThroughput: 1000000,
    });
    await page.locator('#firmware-file').setInputFiles(dat);
    await page.waitForFunction(
      () =>
        document.querySelector('dialog.firmware-actions[open]') ||
        performance.getEntriesByName('camera-engine-boot').length > 0,
    );
    const launch = page.getByRole('button', { name: 'Launch camera', exact: true });
    if (await launch.isVisible()) await launch.click();
    await page.waitForFunction(
      () => {
        const bar = document.querySelector<HTMLProgressElement>(
          '.loading-assets li[data-state=loading] progress',
        );
        const text = document.querySelector('.loading-assets')?.textContent;
        return (
          bar?.hasAttribute('value') &&
          bar.value > 0 &&
          bar.value < 100 &&
          text?.includes('remaining')
        );
      },
      undefined,
      { timeout: 120000 },
    );
    const transfer = await page.locator('#camera-loading').innerText();
    assert.match(transfer, /MiB.*MiB\/s.*~\d+ s remaining/s);
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    await page.screenshot({ path: new URL('download.png', output).pathname });
    await cdp.send('Network.emulateNetworkConditions', {
      offline: false,
      latency: 0,
      downloadThroughput: -1,
      uploadThroughput: -1,
    });
    await page.waitForFunction(
      () => document.querySelector('#live-label')?.textContent === 'LIVE',
      undefined,
      { timeout: 180000 },
    );
    assert.equal(await page.locator('#camera-loading').isVisible(), false);
    assert.ok(
      await page.locator('#lcd-source').evaluate((canvas: HTMLCanvasElement) =>
        canvas
          .getContext('2d')!
          .getImageData(0, 0, canvas.width, canvas.height)
          .data.some((value) => value > 0),
      ),
    );
    assert.equal(errors.length, 0, errors.join('\n'));
    await page.screenshot({ path: new URL('ready.png', output).pathname });
    await writeFile(
      new URL('result.json', output),
      JSON.stringify({ url, transfer, errors }, null, 2),
    );
    console.log(JSON.stringify({ url, transfer, errors }));
    await context.close();
  }
  // The shared preview URL starts the same real firmware path with fixed inputs.
  const previewUrl = new URL('?scenario=x70-lcd#workspace', url).href;
  const preview = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await preview.routeWebSocket('**/*', () => {});
  let releaseDownload!: () => void;
  const permit = new Promise<void>((resolve) => {
    releaseDownload = resolve;
  });
  await preview.route('**/camera.wasm', async (route) => {
    await permit;
    await route.continue();
  });
  const previewPage = await preview.newPage();
  previewPage.on('pageerror', (error) => errors.push(error.stack ?? error.message));
  previewPage.on('console', (message) => {
    if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
  });
  await previewPage.goto(previewUrl);
  await previewPage.waitForFunction(
    () =>
      document.querySelectorAll('.loading-assets li').length >= 3 &&
      document.querySelector('.loading-assets li[data-state=complete]'),
    undefined,
    { timeout: 120000 },
  );
  assert.equal(
    await previewPage.locator('.loading-assets li[data-asset$="camera.wasm"]').count(),
    1,
  );
  await previewPage.locator('#workspace').scrollIntoViewIfNeeded();
  await previewPage.screenshot({ path: new URL('preview-loading.png', output).pathname });
  assert.equal(await previewPage.locator('.loading-assets').isVisible(), true);
  const loadingAssets = await previewPage.locator('.loading-assets').innerText();
  releaseDownload();
  await previewPage.waitForFunction(
    () => document.querySelector('#live-label')?.textContent === 'LIVE',
    undefined,
    { timeout: 180000 },
  );
  assert.equal(errors.length, 0, errors.join('\n'));
  await previewPage.locator('#workspace').scrollIntoViewIfNeeded();
  await previewPage.screenshot({ path: new URL('preview.png', output).pathname });
  await writeFile(
    new URL('preview.json', output),
    JSON.stringify({ url: previewUrl, loadingAssets, errors }, null, 2),
  );
  console.log(JSON.stringify({ url: previewUrl, errors }));
} finally {
  await browser.close();
}
