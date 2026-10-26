import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the prepared camera server.');
const url = new URL('/__wasm-phases', base).href;
const output = new URL('../build/wasm-phases/', import.meta.url);
await mkdir(output, { recursive: true });
const failures: string[] = [];
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
    for (const [index, photo] of (result.photos ?? []).entries()) {
      const encoded = await page.evaluate(async (url: string) => {
        const blob = await (await fetch(url)).blob();
        return new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () =>
            typeof reader.result === 'string'
              ? resolve(reader.result)
              : reject(Error('Missing JPEG data.'));
          reader.onerror = () => reject(reader.error);
          reader.readAsDataURL(blob);
        });
      }, photo.url);
      await writeFile(
        new URL(`${type.name()}-photo-${index + 1}.jpg`, output),
        Buffer.from(encoded.slice(encoded.indexOf(',') + 1), 'base64'),
      );
      delete photo.url;
    }
    await page.screenshot({ path: new URL(`${type.name()}.png`, output).pathname, fullPage: true });
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ url, result, requests, errors }, null, 2) + '\n',
    );
    assert.equal(
      await page.locator('body').getAttribute('data-proof'),
      'passed',
      JSON.stringify({
        error: result.error,
        phases: result.phases?.map(
          ({
            name,
            speed,
            completed,
            error,
          }: {
            name: string;
            speed: number;
            completed: boolean;
            error?: string;
          }) => ({ name, speed, completed, error }),
        ),
      }),
    );
    assert.deepEqual(errors, []);
    assert.deepEqual(
      requests.filter((request) =>
        /pyodide|sources\.zip|_native\.so/i.test(new URL(request).pathname),
      ),
      [],
    );
    console.log(`${type.name()}: every warm operating phase reached at least 1.0x.`);
  } catch (error) {
    failures.push(`${type.name()}: ${String(error)}`);
  } finally {
    await browser.close();
  }
}

assert.deepEqual(failures, []);
