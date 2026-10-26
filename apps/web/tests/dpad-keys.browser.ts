/** Press each D-pad direction where pointer automation aims and check the camera receives it. */
import { chromium, webkit, type Page } from 'playwright';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import type { WasmRequest } from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
declare global {
  interface Window {
    dpadRequests: string[];
  }
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
// The X-A2 first start opens on a language list whose highlight the selector moves.
const firmware = process.env.FUJIMOD_DPAD_FIRMWARE ?? 'xa2-130';
const url = new URL(`/?scenario=camera-rec&firmware=${firmware}#workspace`, base).href;
const output = new URL(`../build/dpad-keys/${firmware}/`, import.meta.url);
await mkdir(output, { recursive: true });
const directions = ['selector.up', 'selector.down', 'selector.left', 'selector.right'];

async function frame(page: Page) {
  const pixels = await page
    .locator('#lcd-source')
    .evaluate((canvas: HTMLCanvasElement) =>
      Array.from(canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height).data),
    );
  return createHash('sha256').update(Uint8Array.from(pixels)).digest('hex');
}

for (const type of [chromium, webkit]) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1400, height: 1100 } });
    await context.routeWebSocket('**/*', () => {});
    await context.addInitScript(() => {
      window.dpadRequests = [];
      const Original = window.Worker;
      window.Worker = class extends Original {
        constructor(...args: ConstructorParameters<typeof Worker>) {
          super(...args);
          this.postMessage = (
            message: unknown,
            options?: Transferable[] | StructuredSerializeOptions,
          ) => {
            const packet = message as { type: string; request?: WasmRequest };
            const request = packet.request;
            if (
              packet.type === 'rpc' &&
              request &&
              (request.action === 'press' || request.action === 'release')
            )
              window.dpadRequests.push(`${request.action} ${request.id}`);
            if (Array.isArray(options)) super.postMessage(message, options);
            else super.postMessage(message, options);
          };
        }
      };
    });
    const page = await context.newPage(),
      errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(url);
    // Every wedge's box center lies on that wedge, not on the MENU/OK key between them.
    await page.locator('.dpad [data-id="selector.down"]').waitFor({ timeout: 300000 });
    const targets = await page.evaluate(
      (ids) =>
        ids.map((id) => {
          const box = document.querySelector(`.dpad [data-id="${id}"]`)!.getBoundingClientRect();
          const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
          return [id, hit?.closest('[data-id]')?.getAttribute('data-id')];
        }),
      directions,
    );
    assert.deepEqual(
      targets,
      directions.map((id) => [id, id]),
    );
    await page.waitForFunction(
      () => (window.cameraPerformance?.guestTicks ?? 0) >= 40 * 12_000_000,
      undefined,
      { timeout: 300000 },
    );
    let presses = 0;
    const press = async (id: string) => {
      // Playwright aims at the box center and refuses a target another element covers.
      await page.locator(`[data-id="${id}"]`).click({ delay: 250 });
      await page.waitForFunction(
        (wanted) => window.dpadRequests.at(-1) === `release ${wanted}`,
        id,
      );
      await page.waitForTimeout(3000);
      await page.locator('#lcd-source').screenshot({
        path: new URL(`${type.name()}-${++presses}-${id}.png`, output).pathname,
      });
      return frame(page);
    };
    const start = await frame(page);
    const down = await press('selector.down');
    assert.notEqual(down, start, 'DOWN left the language list unchanged');
    const up = await press('selector.up');
    // The list stays scrolled after DOWN, so UP is checked against the DOWN frame.
    assert.notEqual(up, down, 'UP did not move the language highlight');
    const ok = await press('menu_ok_button');
    assert.notEqual(ok, up, 'MENU/OK did not confirm the language');
    const requests = await page.evaluate(() => window.dpadRequests);
    assert.deepEqual(
      requests,
      ['selector.down', 'selector.up', 'menu_ok_button'].flatMap((id) => [
        `press ${id}`,
        `release ${id}`,
      ]),
    );
    assert.deepEqual(errors, []);
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ url, targets, requests, frames: { start, down, up, ok } }, null, 2) + '\n',
    );
    console.log(`${type.name()}: ${firmware} D-pad DOWN, UP and MENU/OK reached the camera.`);
  } finally {
    await browser.close();
  }
}
