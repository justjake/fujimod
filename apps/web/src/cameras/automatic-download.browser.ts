/** Test the ordinary launcher against a real mirror through a running proxy Worker. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the development homepage.');
const proxy = process.env.FUJIMOD_TEST_PROXY_URL;
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  await page.context().routeWebSocket('**/*', () => {});
  page.on('pageerror', (error) => console.error('Page error:', error.message));
  const sources: string[] = [];
  await page.route(`${new URL(base).origin}/proxy?*`, async (route) => {
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
  const output = new URL('../../build/automatic-download/', import.meta.url);
  await mkdir(output, { recursive: true });
  await page.locator('.launcher').scrollIntoViewIfNeeded();
  await page.screenshot({ path: new URL('launcher.png', output).pathname });
  await page.locator('.launch-fujimod:not([disabled])').click();
  // Poll from Node: an async browser predicate can be mistaken for a truthy Promise.
  const deadline = Date.now() + 120000;
  let stored = false;
  while (!stored && Date.now() < deadline) {
    stored = await page.evaluate(async () => {
      const module: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      return (await new module.CameraStore().library()).some(
        (entry) => entry.firmwareId === 'x70-110',
      );
    });
    if (!stored) await delay(250);
  }
  assert.ok(stored, 'The verified firmware must be saved before reloading');
  assert.equal(sources.length, 1);
  assert.equal(new URL(sources[0]).hostname, 'fichiers.touslesdrivers.com');
  await page.reload();
  await page.locator('.launch-fujimod:not([disabled])').click();
  await page.locator('.launcher').waitFor({ state: 'detached' });
  assert.equal(sources.length, 1, 'Stored firmware must be reused without another download');
  await page.unrouteAll({ behavior: 'wait' });
  console.log(
    'Passed: Launch Fujimod, real mirror transfer through Worker, verified local import, and reuse.',
  );
} finally {
  await browser.close();
}
