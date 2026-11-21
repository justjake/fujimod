/** Check the camera progress table against the catalog's tagged milestones. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { glob, mkdir, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { byModel, phases, started, type FirmwareProgress } from '../src/ui/camera-progress.ts';

const url = process.env.FUJIMOD_WEB_URL;
const output = process.env.FUJIMOD_TEST_OUTPUT;
if (!url || !output) throw Error('Set the web URL and screenshot output directory.');
const catalog = fileURLToPath(new URL('../../../catalog/progress/', import.meta.url));
const documents: FirmwareProgress[] = [];
for await (const path of glob('*/*/progress.generated.json', { cwd: catalog }))
  documents.push(JSON.parse(await readFile(catalog + path, 'utf8')));
const expected = Object.values(byModel(documents)).filter(started);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({
    viewport: { width: 1500, height: 1100 },
  });
  await page.context().routeWebSocket('**/*', () => {});
  await page.goto(new URL('#camera-table', url).href);
  const rows = page.locator('#camera-table .progress-row');
  assert.equal(await rows.count(), expected.length);
  for (const progress of expected) {
    const row = rows.filter({
      has: page.getByText(progress.model, { exact: true }),
    });
    assert.equal(await row.count(), 1, progress.model);
    const groups = phases(progress);
    const total = groups.reduce((sum, group) => sum + group.steps.length, 0);
    const passed = groups.reduce((sum, group) => sum + group.passed, 0);
    const bar = row.getByRole('progressbar');
    assert.equal(await bar.getAttribute('aria-valuemax'), String(total || 1), progress.model);
    assert.equal(await bar.getAttribute('aria-valuenow'), String(passed), progress.model);
    const measured = await bar.locator('.track-group').evaluateAll((segments) =>
      segments.map((segment) => ({
        width: segment.getBoundingClientRect().width,
        fill: segment.querySelector('.fill')!.getBoundingClientRect().width,
      })),
    );
    assert.equal(measured.length, groups.length, progress.model);
    // Divider pixels are decoration. The remaining width belongs to the tagged outcomes.
    const width = measured.reduce((sum, segment) => sum + segment.width, 0);
    const fill = measured.reduce((sum, segment) => sum + segment.fill, 0);
    for (const [index, group] of groups.entries()) {
      const segment = measured[index]!;
      assert(Math.abs(segment.width - (width * group.steps.length) / total) < 0.1, group.label);
      assert(Math.abs(segment.fill - (width * group.passed) / total) < 0.1, group.label);
    }
    assert(Math.abs(fill - (total ? (width * passed) / total : 0)) < 0.2, progress.model);

    await row.locator('.progress-bar').hover();
    const tip = row.locator('.tip');
    await tip.waitFor({ state: 'visible' });
    assert.equal(await page.locator('#camera-table .progress-bar.open').count(), 1, 'one tip open');
    for (const [index, group] of groups.entries()) {
      const details = tip.locator('.tip-group').nth(index);
      assert.match(await details.locator('.tip-label').innerText(), new RegExp(group.label));
      assert.equal(await details.locator('li:not(.todo)').count(), group.passed, progress.model);
    }
    await page.keyboard.press('Escape');
    await tip.waitFor({ state: 'hidden' });
    await page.mouse.move(1, 1);
  }
  // Taps on a phone: each tap moves the one open tip, and a tap elsewhere closes it.
  const phone = await browser.newPage({
    viewport: { width: 390, height: 844 },
    hasTouch: true,
  });
  await phone.context().routeWebSocket('**/*', () => {});
  await phone.goto(new URL('#camera-table', url).href);
  const tips = phone.locator('#camera-table .progress-bar.open');
  const bars = phone.locator('#camera-table .progress-row .progress-bar');
  for (const index of [0, 1, 2]) {
    await bars.nth(index).tap();
    await bars.nth(index).locator('.tip').waitFor({ state: 'visible' });
    assert.equal(await tips.count(), 1, `tap ${index}`);
    await phone.touchscreen.tap(1, 1);
    assert.equal(await tips.count(), 0, 'a tap elsewhere closes the tip');
  }
  await page.locator('#camera-table').screenshot({ path: `${output}/camera-progress.png` });
  console.log(`${expected.length} started cameras: progress rows and popovers verified at ${url}`);
} finally {
  await browser.close();
}
