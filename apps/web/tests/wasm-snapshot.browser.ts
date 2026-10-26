import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the development server.');
const output = new URL('../build/wasm-snapshot/', import.meta.url);
await mkdir(output, { recursive: true });
for (const type of [chromium, webkit]) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ acceptDownloads: true });
    const page = await context.newPage();
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(new URL('/__wasm-snapshot', base).href);
    await page.waitForFunction(() => document.body?.dataset.proof, undefined, { timeout: 180000 });
    const text = await page.locator('pre').innerText();
    assert.equal(await page.locator('body').getAttribute('data-proof'), 'passed', text);
    const proof = JSON.parse(text) as {
      before: { sha256: string };
      expected: { sha256: string };
      bytes: number;
    };
    const waiting = page.waitForEvent('download');
    await page.getByRole('link', { name: 'Download snapshot' }).click();
    const download = await waiting;
    assert.equal(download.suggestedFilename(), 'snapshot.emusnap');
    const path = new URL(`${type.name()}.emusnap`, output).pathname;
    await download.saveAs(path);
    await page.close();
    const fresh = await context.newPage(),
      requests: string[] = [];
    fresh.on('request', (request) => requests.push(request.url()));
    fresh.on('pageerror', (error) => errors.push(error.message));
    await fresh.goto(new URL('/__wasm-snapshot?load', base).href);
    assert.equal(await fresh.getByLabel('Load snapshot').getAttribute('accept'), '.emusnap');
    await fresh.getByLabel('Load snapshot').setInputFiles(path);
    await fresh.waitForFunction(() => document.body?.dataset.proof, undefined, { timeout: 180000 });
    const resumedText = await fresh.locator('pre').innerText();
    assert.equal(await fresh.locator('body').getAttribute('data-proof'), 'passed', resumedText);
    const resumed = JSON.parse(resumedText) as {
      restored: { sha256: string };
      continued: { sha256: string };
      ticks: number;
    };
    assert.equal(resumed.restored.sha256, proof.before.sha256);
    assert.equal(resumed.continued.sha256, proof.expected.sha256);
    assert.equal(resumed.ticks, 700);
    assert.deepEqual(
      requests.filter((url) => /program\.dat|bootrom|nand\.bin/.test(new URL(url).pathname)),
      [],
    );
    assert.deepEqual(errors, []);
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ proof, resumed, requests }, null, 2),
    );
    console.log(
      `${type.name()}: report snapshot saved to disk, loaded in a fresh page, and resumed at tick 700.`,
    );
  } finally {
    await browser.close();
  }
}
