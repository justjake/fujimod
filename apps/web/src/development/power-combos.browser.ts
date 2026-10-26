/** Review every catalog SD sequence at phone width through the managed development server. */
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the managed web URL.');
const url = new URL('/review/power-combos/', base).href;
const output = new URL('../../build/power-combos/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 375, height: 900 } });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  const response = await page.goto(url);
  assert.equal(response?.status(), 200);
  await page.locator('[data-body="x70"] .power-combo-keys').waitFor();
  assert.equal(await page.locator('[data-body]').count(), 49);
  assert.equal(await page.locator('[data-body="xt5"] h2').innerText(), 'X-T5');
  assert.equal(await page.locator('[data-body="xt30ii"] h2').innerText(), 'X-T30 II');
  assert.equal(await page.locator('[data-body="xq"] h2').innerText(), 'XQ1 / XQ2');
  assert.equal(await page.locator('[data-body="x70"] .rocker .side.glow').count(), 0);
  const glow = await page
    .locator('[data-body="x70"] .rocker .tab.glow')
    .evaluate((element) => getComputedStyle(element).boxShadow);
  assert.ok(glow.includes('255, 255, 255'), glow);
  assert.equal(await page.locator('[data-body="x70"] .rocker .tab.glow').count(), 1);
  assert.equal(await page.locator('[data-body="x70"] .rocker-arrow .push').count(), 1);
  assert.equal(await page.locator('[data-body="x70"] .dial-press').count(), 0);
  assert.equal(await page.locator('[data-body="x70"] .key.rec').count(), 1);
  assert.equal(await page.locator('[data-body="x70"] .seg .on').innerText(), 'ON');
  const overflow = await page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>('.phone, .power-combo-keys')]
      .filter((element) => element.scrollWidth > element.clientWidth + 1)
      .map((element) => element.closest<HTMLElement>('[data-body]')?.dataset.body),
  );
  assert.deepEqual(overflow, []);
  assert.deepEqual(errors, []);
  await page.screenshot({ path: new URL('phone.png', output).pathname, fullPage: true });
  await page.locator('[data-body="x70"]').screenshot({ path: new URL('x70.png', output).pathname });
  console.log(`49 bodies reviewed without overflow or browser errors: ${url}`);
} finally {
  await browser.close();
}
