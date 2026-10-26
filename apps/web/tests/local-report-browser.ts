// Exercise manual feedback and a real main-page replay fault in local development.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL');
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const metadata: { id: string; source: string; event: string; description?: string }[] = [],
    saved: (string | undefined)[] = [];
  page.on('response', (response) => {
    const request = response.request();
    if (request.method() === 'POST' && response.url().endsWith('/__simulator-reports'))
      metadata.push(JSON.parse(request.postData() ?? 'null'));
    if (request.method() === 'PUT' && response.url().includes('/__simulator-reports/')) {
      assert.equal(response.status(), 201);
      saved.push(response.url().split('/').at(-2));
    }
  });
  await page.goto(new URL('?feedback', base).href);
  await page.waitForFunction(() => document.documentElement.dataset.reportsReady === 'true');
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  await page.locator('#feedback-dialog[open]').waitFor();
  await page.locator('#feedback-description').fill('Main-page local feedback integration');
  await page.locator('#feedback-prepare').click();
  await page
    .locator('#feedback-status')
    .filter({ hasText: 'Saved locally' })
    .waitFor({ timeout: 90000 });
  assert.ok(
    metadata.some(
      (report) =>
        report.source === 'emulator' &&
        report.event === 'feedback' &&
        report.description === 'Main-page local feedback integration',
    ),
  );
  await page.goto(new URL('?scenario=x70-movie', base).href);
  const deadline = Date.now() + 240000;
  while (
    !metadata.some((report) => report.source === 'emulator' && report.event === 'crash') &&
    Date.now() < deadline
  )
    await page.waitForTimeout(250);
  assert.ok(metadata.some((report) => report.source === 'emulator' && report.event === 'crash'));
  const crash = metadata.find((report) => report.source === 'emulator' && report.event === 'crash');
  assert.ok(crash);
  const archiveDeadline = Date.now() + 90000;
  while (!saved.includes(crash.id) && Date.now() < archiveDeadline) await page.waitForTimeout(250);
  assert.ok(saved.includes(crash.id));
  console.log('PASS main-page feedback and detected crash saved locally:', saved.join(', '));
} finally {
  await browser.close();
}
