/** Test the ordinary launcher against a real mirror through a running proxy Worker. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the development homepage.');
const proxy = process.env.FUJIMOD_TEST_PROXY_URL;
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  await page.context().routeWebSocket('**/*', () => {});
  const sources: string[] = [];
  await page.route('https://share.fujimod.com/proxy?*', async (route) => {
    const request = new URL(route.request().url());
    sources.push(request.searchParams.get('url') ?? '');
    if (!proxy) return route.continue();
    // Only relocate the service for local testing; the Worker still fetches the real mirror.
    const target = new URL(proxy);
    target.search = request.search;
    const response = await route.fetch({ url: target.href, timeout: 120000 });
    await route.fulfill({ response });
  });
  await page.goto(base);
  const camera = page.getByRole('combobox', { name: 'Camera', exact: true });
  await camera.selectOption({ label: 'X70 · 1.10' });
  const output = new URL('../../build/automatic-download/', import.meta.url);
  await mkdir(output, { recursive: true });
  await page.locator('.launcher').scrollIntoViewIfNeeded();
  await page.screenshot({ path: new URL('launcher.png', output).pathname });
  await page.getByRole('button', { name: 'Run unmodified in your browser', exact: true }).click();
  await page.waitForFunction(
    async () => {
      const module: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      return (await new module.CameraStore().library()).some(
        (entry) => entry.firmwareId === 'x70-110',
      );
    },
    undefined,
    { timeout: 120000 },
  );
  assert.equal(sources.length, 1);
  assert.equal(new URL(sources[0]).hostname, 'fichiers.touslesdrivers.com');
  await page.reload();
  await camera.selectOption({ label: 'X70 · 1.10' });
  await page.getByRole('button', { name: 'Run unmodified in your browser', exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector('.launcher') ||
      document.querySelector('.camera-storage-status')?.textContent !== '',
  );
  assert.equal(sources.length, 1, 'Stored firmware must be reused without another download');
  console.log(
    'Passed: camera selection, real mirror transfer through Worker, verified local import, and reuse.',
  );
} finally {
  await browser.close();
}
