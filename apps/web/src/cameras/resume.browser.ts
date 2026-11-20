/** Real firmware execution: Launch Fujimod creates one camera and resumes it afterward. */
import assert from 'node:assert/strict';
import { chromium, webkit } from 'playwright';
import { mkdir } from 'node:fs/promises';
import { serveFirmware } from './launch.playwright.ts';
const base = process.env.FUJIMOD_WEB_URL,
  dat = process.env.FUJIMOD_TEST_DAT;
assert.ok(base && dat, 'Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/camera-resume/', import.meta.url);
await mkdir(output, { recursive: true });
for (const engine of [chromium, webkit].filter(
  (engine) =>
    !process.env.FUJIMOD_TEST_BROWSER || process.env.FUJIMOD_TEST_BROWSER === engine.name(),
)) {
  const browser = await engine.launch();
  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 1000 } });
    await context.routeWebSocket('**/*', () => {});
    await serveFirmware(context, dat);
    const page = await context.newPage();
    page.setDefaultTimeout(60000);
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (/\[(STRICT_|REACTIVE_|LOADING_|HOT_SCOPE_)/.test(message.text()))
        errors.push(message.text());
    });
    await page.goto(base);
    await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
    const run = page.locator('.launch-fujimod:not([disabled])');
    async function records() {
      return page.evaluate(async () => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        return new CameraStore().list();
      });
    }
    async function live() {
      await page.waitForFunction(
        () =>
          document.querySelector('#live-label')?.textContent === 'LIVE' ||
          document.querySelector('#status')?.classList.contains('error'),
        undefined,
        { timeout: 240000 },
      );
      assert.equal(
        await page.locator('#live-label').innerText(),
        'LIVE',
        await page.locator('#fault-raw').innerText(),
      );
      assert.equal(await page.locator('#camera-loading').isVisible(), false);
    }
    async function exit() {
      await page.getByRole('button', { name: 'Close', exact: true }).click();
      await run.waitFor();
    }
    assert.equal((await records()).length, 0, 'Opening the page never creates a camera');
    await run.click();
    await live();
    const first = (await records())[0];
    assert.equal(first.modEnabled, true);
    await exit();
    const before = await records();
    await run.click();
    await live();
    await exit();
    const after = await records();
    assert.deepEqual(
      after.map((record) => record.id),
      [first.id],
    );
    assert.ok(after[0].revision > before[0].revision);
    assert.deepEqual(errors, []);
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    await page.screenshot({ path: new URL(`${engine.name()}.png`, output).pathname });
    console.log(`${engine.name()}: real Fujimod boot, save and resume passed`);
  } finally {
    await browser.close();
  }
}
