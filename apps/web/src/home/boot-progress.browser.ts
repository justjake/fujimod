/** Verify the loading row during real execution in the development preview. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const url = new URL('?scenario=x70-lcd#workspace', base).href;
const output = new URL('../../build/boot-progress/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  // Keep this real boot on one source revision while development continues.
  await context.routeWebSocket('**/*', () => {});
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(url);
  await page.waitForFunction(
    () => {
      const row = document.querySelector('[data-asset="step:boot"]');
      const progress = row?.querySelector('progress');
      return (
        row?.getAttribute('data-state') === 'loading' &&
        progress?.hasAttribute('value') &&
        progress.value > 0 &&
        progress.value <= 99 &&
        /\d+%/.test(row.textContent ?? '')
      );
    },
    undefined,
    { timeout: 240000 },
  );
  const row = page.locator('[data-asset="step:boot"]');
  const first = await row.innerText();
  assert(await page.locator('#card-section').isHidden(), 'The SD card must stay hidden until the camera runs.');
  await page.screenshot({ path: new URL('booting.png', output).pathname, fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await row.scrollIntoViewIfNeeded();
  const fits = await row.evaluate((element) => {
    const spans = element.querySelectorAll('span');
    const label = document.createRange(),
      count = document.createRange();
    label.selectNodeContents(spans[0]);
    count.selectNodeContents(spans[1]);
    return (
      label.getBoundingClientRect().right <= count.getBoundingClientRect().left + 1 &&
      count.getBoundingClientRect().right <= element.getBoundingClientRect().right + 1
    );
  });
  assert(fits, 'Boot label and percentage must fit without overlap on a phone.');
  await page.screenshot({ path: new URL('booting-mobile.png', output).pathname, fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.waitForFunction(
    (previous) => document.querySelector('[data-asset="step:boot"]')?.textContent !== previous,
    await row.textContent(),
    { timeout: 30000 },
  );
  await page.waitForFunction(
    () => document.querySelector('#live-label')?.textContent === 'LIVE',
    undefined,
    { timeout: 240000 },
  );
  assert.deepEqual(errors, []);
  await page.locator('#card-section').waitFor({ state: 'visible', timeout: 30000 });
  await page.screenshot({ path: new URL('rec.png', output).pathname, fullPage: true });
  await writeFile(new URL('result.json', output), JSON.stringify({ url, first, errors }, null, 2));
  console.log(first);
  console.log('Real boot reached REC with determinate tick progress.');
} finally {
  await browser.close();
}
