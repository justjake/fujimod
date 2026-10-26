/** Exercise the settled app route through its actual generated control panel. */
import type { WasmRequest } from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
declare global {
  interface Window {
    qualifiedPositions?: Record<string, string>;
    qualifiedInputs: WasmRequest[];
  }
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const url = new URL('/?scenario=x70-lcd#workspace', base).href;
const output = new URL('../build/engine-ui/', import.meta.url);
await mkdir(output, { recursive: true });
for (const type of [chromium, webkit].filter(
  (type) => !process.env.FUJIMOD_TEST_BROWSER || type.name() === process.env.FUJIMOD_TEST_BROWSER,
)) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1300, height: 1100 } });
    await context.routeWebSocket('**/*', () => {});
    await context.addInitScript(() => {
      window.qualifiedInputs = [];
      const Original = window.Worker;
      window.Worker = class extends Original {
        constructor(...args: ConstructorParameters<typeof Worker>) {
          super(...args);
          this.postMessage = (
            message: unknown,
            options?: Transferable[] | StructuredSerializeOptions,
          ) => {
            const packet = message as { type: string; request?: WasmRequest };
            if (
              packet.type === 'rpc' &&
              packet.request &&
              ['rotate', 'touch', 'set_position'].includes(packet.request.action)
            )
              window.qualifiedInputs.push(packet.request);
            if (Array.isArray(options)) super.postMessage(message, options);
            else super.postMessage(message, options);
          };
          this.addEventListener('message', ({ data }) => {
            if (data.type === 'state') window.qualifiedPositions = data.state.positions;
          });
        }
      };
    });
    const page = await context.newPage(),
      errors: string[] = [];
    page.on('pageerror', (e) => {
      errors.push(e.message);
      console.error(e.message);
    });
    page.on('console', (message) => console.log(type.name() + ': ' + message.text()));
    await page.goto(url);
    console.log(type.name() + ': app loaded');
    await page.waitForFunction(() => window.cameraPerformance, undefined, { timeout: 300000 });
    const result: Record<string, unknown> = { url };
    const canvas = page.locator('#lcd-source');
    const rec = await canvas.evaluate((c: HTMLCanvasElement) => c.toDataURL());
    await page.locator('button[data-id="menu_ok_button"]').click();
    await page.waitForFunction(
      (old) => document.querySelector<HTMLCanvasElement>('#lcd-source')?.toDataURL() !== old,
      rec,
      { timeout: 60000 },
    );
    await page.screenshot({
      path: new URL(type.name() + '-menu.png', output).pathname,
      fullPage: true,
    });
    result.undo = await page.evaluate(async () => {
      const before = await window.cameraEngine!.profile(0);
      await window.cameraEngine!.rollback(10);
      return { before, after: await window.cameraEngine!.profile(0) };
    });
    await page.locator('[data-id="power_switch"] [data-value="OFF"]').click();
    await page.waitForFunction(
      () => document.querySelector<HTMLCanvasElement>('#lcd-source')?.style.visibility === 'hidden',
      undefined,
      { timeout: 60000 },
    );
    const stopped = await page.evaluate(() => window.cameraEngine!.profile(0));
    await page.waitForTimeout(300);
    assert.equal((await page.evaluate(() => window.cameraEngine!.profile(0))).ticks, stopped.ticks);
    await page.locator('[data-id="power_switch"] [data-value="ON"]').click();
    await page.waitForFunction(
      () =>
        document.querySelector<HTMLCanvasElement>('#lcd-source')?.style.visibility !== 'hidden' &&
        document
          .querySelector('[data-id="shutter_speed_dial"][role="slider"]')
          ?.getAttribute('aria-disabled') === 'false',
      undefined,
      { timeout: 300000 },
    );
    for (const direction of ['ArrowRight', 'ArrowLeft']) {
      const ring = page.locator('[data-id="control_ring"][role="slider"]');
      const before = await page.evaluate(() => window.cameraEngine!.history());
      await ring.focus();
      await ring.press(direction);
      await page.waitForFunction(
        async (count) => (await window.cameraEngine!.history()).entries > count,
        before.entries,
      );
      await page.screenshot({
        path: new URL(type.name() + '-ring-' + direction + '.png', output).pathname,
        fullPage: true,
      });
    }
    const rect = await canvas.evaluate((canvas: HTMLCanvasElement) => {
      const b = canvas.getBoundingClientRect(),
        scale = Math.min(b.width / canvas.width, b.height / canvas.height);
      return {
        x: b.x + (b.width - canvas.width * scale) / 2,
        y: b.y + (b.height - canvas.height * scale) / 2,
        width: canvas.width * scale,
        height: canvas.height * scale,
      };
    });
    const pointer = {
      x: Math.round(rect.x + (rect.width * 300) / 720),
      y: Math.round(rect.y + (rect.height * 200) / 480),
    };
    const expectedTouch = {
      x: Math.floor(((pointer.x - rect.x) * 720) / rect.width),
      y: Math.floor(((pointer.y - rect.y) * 480) / rect.height),
    };
    await page.mouse.move(pointer.x, pointer.y);
    await page.mouse.down();
    await page.waitForTimeout(100);
    await page.mouse.move(rect.x + (rect.width * 420) / 720, rect.y + (rect.height * 260) / 480, {
      steps: 2,
    });
    await page.waitForTimeout(100);
    await page.mouse.up();
    await page.evaluate(() => window.cameraEngine!.touch('touchscreen', 'tap', 300, 200));
    const admitted = await page.evaluate(() => window.qualifiedInputs);
    result.physicalInputs = admitted.filter((r) =>
      ['rotate', 'touch', 'set_position'].includes(r.action),
    );
    assert.deepEqual(
      admitted.filter((r) => r.action === 'rotate').map((r) => r.steps),
      [1, -1],
    );
    const touches = admitted.filter((r) => r.action === 'touch');
    assert.deepEqual(
      [...new Set(touches.map((r) => r.phase))],
      ['press', 'drag', 'release', 'tap'],
    );
    assert.equal(touches[0].x, expectedTouch.x);
    assert.equal(touches[0].y, expectedTouch.y);
    assert.deepEqual(touches.at(-1), {
      action: 'touch',
      id: 'touchscreen',
      phase: 'tap',
      x: 300,
      y: 200,
    });
    assert.deepEqual(
      admitted.flatMap((r) =>
        r.action === 'set_position' && r.id === 'power_switch' ? [r.position] : [],
      ),
      ['OFF', 'ON'],
    );
    const dial = page.locator('[data-id="shutter_speed_dial"][role="slider"]');
    await dial.focus();
    await dial.press('ArrowRight');
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-id="shutter_speed_dial"][role="slider"]')
          ?.getAttribute('aria-valuenow') === '1',
    );
    for (const position of ['normal', 'selfie']) {
      await page.locator(`[data-id="lcd_tilt_switch"] [data-value="${position}"]`).click();
      await page.waitForFunction(
        (value) => window.qualifiedPositions?.lcd_tilt_switch === value,
        position,
      );
      assert.equal(
        await page
          .locator(`[data-id="lcd_tilt_switch"] [data-value="${position}"]`)
          .getAttribute('aria-pressed'),
        'true',
      );
      await page.screenshot({
        path: new URL(type.name() + '-hinge-' + position + '.png', output).pathname,
        fullPage: true,
      });
    }
    result.profile = await page.evaluate(() => window.cameraEngine!.profile(5));
    result.errors = errors;
    await page.screenshot({ path: new URL(type.name() + '.png', output).pathname, fullPage: true });
    await writeFile(new URL(type.name() + '.json', output), JSON.stringify(result, null, 2) + '\n');
    assert.deepEqual(errors, []);
    assert.equal((result.profile as { failed: boolean }).failed, false);
    console.log(
      type.name() +
        ': app MENU, native Undo, both power choices, shutter dial and both LCD hinge positions passed.',
    );
  } finally {
    await browser.close();
  }
}
