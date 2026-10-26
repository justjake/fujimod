import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the prepared camera server.');
const url = new URL('/__wasm-lifecycle', base).href;
const output = new URL('../build/wasm-lifecycle/', import.meta.url);
await mkdir(output, { recursive: true });
for (const type of [chromium, webkit].filter(
  (type) => !process.env.FUJIMOD_TEST_BROWSER || type.name() === process.env.FUJIMOD_TEST_BROWSER,
)) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1200, height: 1000 } });
    await context.routeWebSocket('**/*', () => {});
    const page = await context.newPage(),
      errors: string[] = [],
      requests: string[] = [];
    page.on('console', (message) => console.log(type.name() + ': ' + message.text()));
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => requests.push(request.url()));
    await page.goto(url);
    await page.waitForFunction(() => document.body.dataset.proof, undefined, { timeout: 1200000 });
    const result = JSON.parse(await page.locator('pre').innerText());
    await page.screenshot({ path: new URL(`${type.name()}.png`, output).pathname, fullPage: true });
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ url, result, requests, errors }, null, 2) + '\n',
    );
    assert.equal(
      await page.locator('body').getAttribute('data-proof'),
      'passed',
      JSON.stringify(result),
    );
    assert.deepEqual(errors, []);
    assert.deepEqual(
      requests.filter((request) =>
        /pyodide|sources\.zip|_native\.so/i.test(new URL(request).pathname),
      ),
      [],
    );
    console.log(
      `${type.name()}: owned save/reload, shutter JPEG, Playback, power, held boot, merge, format, report and Undo passed.`,
    );
  } finally {
    await browser.close();
  }
}
