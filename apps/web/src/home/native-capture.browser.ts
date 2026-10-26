/** Replay real shutter input through the served native prepared X70 bundle. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the managed preview URL.');
const url = new URL('?scenario=x70-photo#workspace', base).href;
const output = new URL('../../build/native-capture/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.stack ?? error.message));
  const manifests: string[] = [];
  page.on('response', (response) => {
    if (response.url().endsWith('/x70-110/wasm.json')) manifests.push(response.url());
  });
  await page.goto(url);
  await page.waitForFunction(
    () =>
      /DSCF\d+\.JPG/i.test(document.querySelector('#card-files')?.textContent ?? '') ||
      document.querySelector('#status')?.classList.contains('error'),
    undefined,
    { timeout: 300000 },
  );
  assert.ok(manifests.length > 0, 'The served X70 manifest must be loaded');
  const manifest: unknown = await (await page.request.get(manifests[0]!)).json();
  assert.ok(
    manifest !== null &&
      typeof manifest === 'object' &&
      'construction' in manifest &&
      manifest.construction === 'prepared',
  );
  const files = await page.locator('#card-files').innerText();
  assert.match(files, /DSCF\d+\.JPG/);
  assert.deepEqual(errors, []);
  const download = page.waitForEvent('download');
  await page
    .getByRole('button', { name: /^Download DSCF\d+\.JPG$/ })
    .first()
    .click();
  const file = await download;
  await file.saveAs(new URL('capture.jpg', output).pathname);
  await page.locator('#workspace').scrollIntoViewIfNeeded();
  await page.locator('#workspace').screenshot({ path: new URL('browser.png', output).pathname });
  await writeFile(
    new URL('receipt.json', output),
    JSON.stringify({ url, files, manifests, errors }, null, 2),
  );
  console.log(JSON.stringify({ url, files, errors }));
} finally {
  for (const context of browser.contexts())
    for (const page of context.pages())
      await page.locator('#workspace').screenshot({ path: new URL('browser.png', output).pathname });
  await browser.close();
}
