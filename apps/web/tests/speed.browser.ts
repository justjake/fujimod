/** Measure guest time from real worker replies, with live pixels and Undo enabled. */
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';

interface ObservedState {
  inputQueue?: { completed: number; pending: number; active: unknown };
}
declare global {
  interface Window {
    speedTestState?: ObservedState;
    speedTestFault?: unknown;
  }
}

const { values } = parseArgs({
  options: {
    url: { type: 'string' },
    dat: { type: 'string' },
    output: { type: 'string' },
    engine: { type: 'string', default: 'chromium' },
    seconds: { type: 'string', default: '30' },
    'warmup-guest-seconds': { type: 'string', default: '25' },
    'minimum-speed': { type: 'string', default: '1.0' },
  },
});
assert.ok(values.url && values.output, 'Supply --url and a fresh --output directory.');
assert.ok(values.engine === 'chromium' || values.engine === 'webkit');
const seconds = Number(values.seconds);
const warmup = Number(values['warmup-guest-seconds']);
const minimumSpeed = Number(values['minimum-speed']);
assert.ok(Number.isFinite(seconds) && seconds >= 5);
assert.ok(Number.isFinite(warmup) && warmup >= 0);
assert.ok(Number.isFinite(minimumSpeed) && minimumSpeed > 0);
const output = resolve(values.output);
await mkdir(output, { recursive: false });
const browser = await (values.engine === 'chromium' ? chromium : webkit).launch({
  headless: false,
});
const context = await browser.newContext({ viewport: { width: 1300, height: 1000 } });
// Development reloads must not replace the runtime halfway through a measurement.
await context.routeWebSocket('**/*', () => {});
await context.addInitScript(() => {
  const Original = window.Worker;
  window.Worker = class extends Original {
    constructor(...args: ConstructorParameters<typeof Worker>) {
      super(...args);
      if (!String(args[0]).includes('wasm-worker')) return;
      this.addEventListener(
        'message',
        ({
          data,
        }: MessageEvent<{
          type: string;
          state?: ObservedState;
          fault?: unknown;
          error?: string;
        }>) => {
          if (data.state) window.speedTestState = { ...window.speedTestState, ...data.state };
          if (data.type === 'error') window.speedTestFault = data.fault ?? data.error;
        },
      );
    }
  };
});
const page = await context.newPage();
const errors: string[] = [],
  consoleMessages: string[] = [];
page.on('pageerror', (error) => errors.push(error.message));
page.on('console', (message) => consoleMessages.push(`${message.type()}: ${message.text()}`));
const result: Record<string, unknown> = {
  url: values.url,
  engine: values.engine,
  browser: browser.version(),
};
try {
  const response = await page.goto(values.url);
  result.csp = response?.headers()['content-security-policy'];
  assert.match(String(result.csp), /'wasm-unsafe-eval'/);
  await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length > 0);
  if (values.dat) await page.locator('#firmware-file').setInputFiles(values.dat);
  await page.waitForFunction(() => window.cameraPerformance || window.speedTestFault, undefined, {
    timeout: 240000,
  });
  assert.equal(await page.evaluate(() => window.speedTestFault), undefined);
  const ready = await page.evaluate(() => window.cameraPerformance!.guestTicks);
  console.log('Camera ready; warming guest execution.');
  await page.waitForFunction(
    (target) => window.speedTestFault || window.cameraPerformance!.guestTicks >= target,
    ready + warmup * 12000000,
    { timeout: 600000 },
  );
  result.warmupFault = await page.evaluate(() => window.speedTestFault);
  assert.equal(result.warmupFault, undefined);
  async function sample() {
    return page.evaluate(() => ({
      ticks: window.cameraPerformance!.guestTicks,
      frames: Number(window.cameraPerformance!.runtime?.sourceFrames ?? 0),
      wall: performance.now(),
      presented: Number(window.cameraPerformance?.runtime?.presentedImageFrames ?? 0),
      metrics: window.cameraPerformance,
    }));
  }
  const warmedProfile = await page.evaluate(() => window.cameraEngine!.profile(0));
  result.jit = warmedProfile.jit;
  const jit = warmedProfile.jit as {
    enabled: boolean;
    compiled: number;
    installed: number;
    failures: number;
    unsupported: number;
    compiledExecutions: string;
  };
  assert.equal(jit.enabled, true);
  assert.ok(jit.compiled > 0 && jit.installed > 0 && BigInt(jit.compiledExecutions) > 0n);
  assert.equal(jit.failures, 0);
  assert.equal(jit.unsupported, 0);
  const before = await sample();
  console.log(`Measuring ${seconds} wall seconds.`);
  await page.waitForTimeout(seconds * 1000);
  const after = await sample();
  const guestSeconds = (after.ticks - before.ticks) / 12000000;
  const speed = guestSeconds / ((after.wall - before.wall) / 1000);
  const sourceFps = (after.frames - before.frames) / guestSeconds;
  const presentedFps = (after.presented - before.presented) / guestSeconds;
  Object.assign(result, {
    before,
    after,
    speed,
    sourceFps,
    presentedFps,
    warmupGuestSeconds: warmup,
    minimumSpeed,
    errors,
    fault: await page.evaluate(() => window.speedTestFault),
  });
  assert.equal(await page.evaluate(() => window.speedTestFault), undefined);
  await page.locator('#viewer').scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${output}/shooting.png` });
  const oldPixels = await page
    .locator('#lcd-source')
    .evaluate((canvas: HTMLCanvasElement) => canvas.toDataURL());
  const completed = await page.evaluate(() => window.speedTestState?.inputQueue?.completed ?? 0);
  const started = performance.now();
  await page.locator('button[data-id="menu_ok_button"]').click();
  await page.waitForFunction(
    (count) => {
      const queue = window.speedTestState?.inputQueue;
      return (
        window.speedTestFault ||
        (queue && queue.completed >= count + 2 && !queue.active && !queue.pending)
      );
    },
    completed,
    { timeout: 60000 },
  );
  await page.waitForFunction(
    (old) => {
      const canvas = document.querySelector<HTMLCanvasElement>('#lcd-source');
      return window.speedTestFault || canvas?.toDataURL() !== old;
    },
    oldPixels,
    { timeout: 60000 },
  );
  result.menuSeconds = (performance.now() - started) / 1000;
  await page.locator('#viewer').scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${output}/menu.png` });
  const history = await page.evaluate(async () => {
    const profile = await window.cameraEngine!.profile();
    const log = await window.cameraEngine!.history();
    return { checkpoints: profile.checkpoints, entries: log.entries, bytes: log.log.length };
  });
  result.history = history;
  assert.ok(Array.isArray(history.checkpoints) && history.checkpoints.length >= 2);
  assert.ok(history.bytes > 0);
  const beforeUndo = await page.evaluate(() => window.cameraPerformance!.guestTicks);
  await page.evaluate(() => window.cameraEngine!.rollback(10));
  await page.waitForFunction((tick) => window.cameraPerformance!.guestTicks < tick, beforeUndo);
  result.undoTicks = await page.evaluate(() => window.cameraPerformance!.guestTicks);
  assert.equal(await page.evaluate(() => window.speedTestFault), undefined);
  assert.deepEqual(errors, []);
  assert.ok(sourceFps >= 9.5, `${sourceFps} source frames per guest second`);
  assert.ok(presentedFps >= 9.5, `${presentedFps} presented frames per guest second`);
  assert.ok(speed >= minimumSpeed, `${speed.toFixed(3)}x < ${minimumSpeed}x`);
} catch (error) {
  result.console = consoleMessages;
  result.lastMetrics = await page.evaluate(() => window.cameraPerformance);
  result.fault = await page.evaluate(() => window.speedTestFault);
  result.error = String(error);
  throw error;
} finally {
  await writeFile(`${output}/result.json`, JSON.stringify(result, null, 2) + '\n');
  console.log(JSON.stringify(result));
  await browser.close();
}
