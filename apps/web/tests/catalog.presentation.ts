/** Check the camera progress table against the catalog's generated progress and publishing overrides. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { glob, mkdir, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import {
  byModel,
  phases,
  started,
  withOverrides,
  type FirmwareProgress,
  type ProgressOverride,
} from '../src/ui/camera-progress.ts';

const url = process.env.FUJIMOD_WEB_URL;
const output = process.env.FUJIMOD_TEST_OUTPUT;
if (!url || !output) throw Error('Set the web URL and screenshot output directory.');
const catalog = fileURLToPath(new URL('../../../catalog/progress/', import.meta.url));
const documents: FirmwareProgress[] = [];
for await (const path of glob('*/*/progress.generated.json', { cwd: catalog }))
  documents.push(JSON.parse(await readFile(catalog + path, 'utf8')));
const overrides: ProgressOverride[] = [];
for await (const path of glob('*/*/progress.override.json', { cwd: catalog }))
  overrides.push(JSON.parse(await readFile(catalog + path, 'utf8')));
const expected = Object.values(byModel(withOverrides(documents, overrides))).filter(started);
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
    const list = phases(progress);
    for (const [index, phase] of list.entries()) {
      const segment = row.locator('.phase').nth(index);
      await segment.hover();
      const tip = segment.locator('.tip');
      await tip.waitFor({ state: 'visible' });
      assert.equal(await tip.locator('.tip-label').innerText(), phase.label, progress.model);
      assert.equal(await page.locator('#camera-table .phase.open').count(), 1, 'one tip open');
      assert.equal(
        await tip.locator('li:not(.todo)').count(),
        phase.steps.filter((step) => step.fraction === 1).length,
        `${progress.model} ${phase.label}`,
      );
    }
  }
  // Taps on a phone: each tap moves the one open tip, and a tap elsewhere closes it.
  const phone = await browser.newPage({
    viewport: { width: 390, height: 844 },
    hasTouch: true,
  });
  await phone.context().routeWebSocket('**/*', () => {});
  await phone.goto(new URL('#camera-table', url).href);
  const tips = phone.locator('#camera-table .phase.open');
  const segments = phone.locator('#camera-table .progress-row .phase:nth-child(2)');
  for (const index of [0, 1, 2]) {
    await segments.nth(index).tap();
    await segments.nth(index).locator('.tip').waitFor({ state: 'visible' });
    assert.equal(await tips.count(), 1, `tap ${index}`);
  }
  await phone.locator('#camera-table h2').tap();
  assert.equal(await tips.count(), 0, 'a tap elsewhere closes the tip');
  await page.locator('#camera-table').screenshot({ path: `${output}/camera-progress.png` });
  console.log(`${expected.length} started cameras: progress rows and popovers verified at ${url}`);
} finally {
  await browser.close();
}
