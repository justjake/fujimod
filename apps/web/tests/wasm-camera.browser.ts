import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import type { Fingerprint } from 'fujimod/emulator';
import { mkdir, writeFile, readFile } from 'node:fs/promises';
const base = process.env.FUJIMOD_WEB_URL;
const receipt = process.env.FUJIMOD_WASM_EXPECT_EXECUTION;
if (!base || !receipt)
  throw Error(
    'Set FUJIMOD_WEB_URL and FUJIMOD_WASM_EXPECT_EXECUTION to the prepared camera and matching execution receipt.',
  );
const expected = JSON.parse(await readFile(receipt, 'utf8')) as {
  created: boolean;
  error: string | null;
  status: { ticks: string; cores: string[]; failed: boolean };
  frame: { width: number; height: number; sha256: string };
  fingerprint: Fingerprint;
};
assert(expected.created && !expected.error && !expected.status.failed && expected.frame);
const url = new URL('/__wasm-camera', base).href;
const manifestResponse = await fetch(new URL('/__wasm-camera/wasm.json', base));
assert(manifestResponse.ok);
const manifest = (await manifestResponse.json()) as { dmpCpuRenderer?: boolean };
const output = new URL('../build/wasm-camera/', import.meta.url);
await mkdir(output, { recursive: true });
for (const type of manifest.dmpCpuRenderer === false ? [chromium] : [chromium, webkit]) {
  const browser = await type.launch({
    headless: true,
    ...(type === chromium ? { args: ['--enable-unsafe-webgpu'] } : {}),
  });
  try {
    const context = await browser.newContext({ viewport: { width: 1200, height: 1000 } });
    await context.routeWebSocket('**/*', () => {});
    const page = await context.newPage(),
      errors: string[] = [],
      requests: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => requests.push(request.url()));
    await page.goto(url);
    await page.waitForFunction(() => document.body.dataset.proof, undefined, { timeout: 600000 });
    const result = JSON.parse(await page.locator('pre').innerText());
    await page.screenshot({
      path: new URL(`${type.name()}.png`, output).pathname,
      fullPage: false,
    });
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
    assert.equal(result.datSha256, process.env.FUJIMOD_WASM_DAT_SHA256);
    assert.equal(String(result.execution.profile.ticks), expected.status.ticks);
    assert.deepEqual(result.execution.profile.instructions.map(String), expected.status.cores);
    assert.equal(result.execution.profile.failed, false);
    const expectedRasterizer = await page.evaluate(async () => {
      const manifest = await (await fetch('/__wasm-camera/wasm.json')).json();
      return manifest.dmpCpuRenderer === false ? 'WebGPU' : 'CPU';
    });
    assert.equal(result.execution.dmpRasterizer, expectedRasterizer);
    assert.deepEqual(result.execution.fingerprint, expected.fingerprint);
    assert.deepEqual(result.execution.frame, {
      width: expected.frame.width,
      height: expected.frame.height,
      sha256: expected.frame.sha256,
    });
    assert.deepEqual(
      requests.filter((request) =>
        /pyodide|sources\.zip|_native\.so/i.test(new URL(request).pathname),
      ),
      [],
    );
    console.log(`${type.name()}: real REC, menu, dial, scene, card and native Undo passed.`);
  } finally {
    await browser.close();
  }
}
