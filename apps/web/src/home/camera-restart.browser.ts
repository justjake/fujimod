/** Check owned-camera identity and reactive updates through real firmware restarts. */
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL,
  dat = process.env.FUJIMOD_TEST_DAT;
if (!base || !dat) throw Error('Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/viewport-controls/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
try {
  const context = await browser.newContext({ viewport: { width: 1200, height: 1000 } });
  await context.routeWebSocket('**/*', () => {});
  const page = await context.newPage(),
    errors: string[] = [];
  page.setDefaultTimeout(60000);
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
  });
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
  const chooser = page.waitForEvent('filechooser');
  await page.locator('#firmware-prompt').click();
  await (await chooser).setFiles(dat);
  async function ready() {
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
      `${await page.locator('#status').innerText()}\n${await page.locator('#fault-raw').innerText()}`,
    );
    await page.waitForFunction(
      () => !document.querySelector<HTMLButtonElement>('#firmware-mode button')?.disabled,
    );
  }
  async function records() {
    return page.evaluate(async () => {
      const { CameraStore }: typeof import('../cameras/store') = await import(
        '/src/' + 'cameras/store.ts'
      );
      return new CameraStore().list();
    });
  }
  await ready();
  const id = (await records())[0].id;
  for (const label of ['Unmodded', 'FujiMod']) {
    await page.locator('#firmware-mode > button').click();
    await page.locator(`[data-mod-enabled="${label === 'FujiMod'}"]`).click();
    await ready();
    const saved = await records();
    assert.equal(saved.length, 1);
    assert.equal(saved[0].id, id);
    assert.equal(await page.locator('#firmware-mode > button').innerText(), label);
    console.log(`${label}: native boot and owned-camera identity passed`);
  }
  assert.deepEqual(errors, []);
  await writeFile(
    new URL('restart-result.json', output),
    JSON.stringify({ id, modes: ['Unmodded', 'FujiMod'], errors }, null, 2),
  );
  console.log('Passed: native firmware restarts preserve one camera with no Solid diagnostics');
} finally {
  await browser.close();
}

const safari = await webkit.launch();
try {
  const page = await safari.newPage({ viewport: { width: 390, height: 844 } }),
    errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  assert.equal(await page.locator('#camera-exit').count(), 0);
  assert.equal(await page.locator('#viewer-tools').isVisible(), false);
  await page.goto(new URL('?feedback', base).href, { waitUntil: 'domcontentloaded' });
  await page.locator('#feedback-dialog[open]').waitFor();
  assert.equal(
    await page
      .locator('#feedback-dialog')
      .evaluate((element) => (element as HTMLDialogElement).open),
    true,
  );
  await page.screenshot({ path: new URL('webkit-feedback.png', output).pathname });
  assert.deepEqual(errors, []);
  console.log('Passed: WebKit first use has no idle menu and opens the feedback dialog');
} finally {
  await safari.close();
}
