import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the development server.');
const url = new URL('/__wasm-proof', base).href;
const output = new URL('../build/wasm-proof/', import.meta.url);
await mkdir(output, { recursive: true });
const types = [chromium, webkit].filter(
  (type) => !process.env.FUJIMOD_TEST_BROWSER || type.name() === process.env.FUJIMOD_TEST_BROWSER,
);
assert(types.length, 'Unknown FUJIMOD_TEST_BROWSER.');
for (const type of types) {
  const browser = await type.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 1000 } });
    const errors: string[] = [],
      requests: string[] = [],
      workers: string[] = [];
    page.on('console', (message) => console.log(type.name() + ': ' + message.text()));
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => requests.push(request.url()));
    page.on('worker', (worker) => workers.push(worker.url()));
    await page.goto(url);
    await page.waitForFunction(() => document.body.dataset.proof, undefined, { timeout: 300000 });
    const text = await page.locator('pre').innerText();
    assert.equal(await page.locator('body').getAttribute('data-proof'), 'passed', text);
    const result = JSON.parse(text) as {
      passed: boolean;
      frames: { sequence: number; pixels: number[] }[];
      metrics: { guestTicks: number; runtime: { instructions: string } };
      faults: { kind: string }[];
      card: { supported: boolean };
      session: { scenes: number[]; profile: { ticks: number }; refusals: Record<string, string> };
    };
    assert(result.passed);
    assert(await page.evaluate(() => crossOriginIsolated && typeof SharedArrayBuffer === 'function'));
    assert.ok(result.frames.length >= 2);
    assert.ok(
      result.frames.some(
        (frame) =>
          JSON.stringify(frame.pixels) === JSON.stringify(Array(4).fill([0, 1, 35, 255]).flat()),
      ),
    );
    assert.deepEqual(result.frames.at(-1)!.pixels, Array(4).fill([0, 4, 86, 255]).flat());
    assert.equal(result.metrics.guestTicks, 205000000);
    assert.equal(result.metrics.runtime.instructions, '205000000');
    assert.equal(result.faults[0]!.kind, 'guest-fault');
    assert.equal(result.card.supported, false);
    assert.deepEqual(result.session.scenes, [0, 1, 2]);
    assert.equal(result.session.profile.ticks, 0);
    assert.deepEqual(Object.keys(result.session.refusals), ['press', 'release', 'set_position']);
    const owners = workers.filter((worker) => new URL(worker).pathname.endsWith('/wasm-worker.ts'));
    const devices = workers.filter((worker) => new URL(worker).pathname.endsWith('/camera.mjs'));
    assert(owners.length >= 3);
    // Chromium reports device pthreads; WebKit reports owners only. An owner
    // can be observed before it imports a module, and each loaded module starts
    // its pthreads in pairs.
    if (type === chromium)
      assert(devices.length >= 2 && devices.length % 2 === 0, 'Device pthreads start in pairs.');
    assert.equal(workers.length, owners.length + devices.length);
    assert.deepEqual(
      requests.filter((request) =>
        /pyodide|numpy|pillow|sources\.zip|_native\.so/i.test(new URL(request).pathname),
      ),
      [],
    );
    assert.deepEqual(errors, []);
    await page.screenshot({ path: new URL(`${type.name()}.png`, output).pathname, fullPage: true });
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ url, result, requests, workers }, null, 2) + '\n',
    );
    console.log(`${type.name()} Wasm execution proof passed: ${url}`);
  } finally {
    await browser.close();
  }
}
