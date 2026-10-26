/** Qualify a retained body's actual app, native catalog and physical controls. */
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import type { ControlInput } from 'fujimod/emulator';
import type { WasmRequest } from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
declare global {
  interface Window {
    retainedInputs: ControlInput[];
    retainedRequests: WasmRequest[];
  }
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const firmware = process.env.FUJIMOD_RETAINED_FIRMWARE ?? 'xm1-150';
const supported = Number(process.env.FUJIMOD_RETAINED_SUPPORTED ?? 11);
const unavailable = Number(process.env.FUJIMOD_RETAINED_UNAVAILABLE ?? 8);
const url = new URL(`/?scenario=camera-rec&firmware=${firmware}#workspace`, base).href;
const output = new URL(`../build/retained-controls/${firmware}/`, import.meta.url);
await mkdir(output, { recursive: true });
for (const type of [chromium, webkit]) {
  const browser = await type.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1300, height: 1100 } });
    await context.routeWebSocket('**/*', () => {});
    await context.addInitScript(() => {
      window.retainedInputs = [];
      window.retainedRequests = [];
      const Original = window.Worker;
      window.Worker = class extends Original {
        constructor(...args: ConstructorParameters<typeof Worker>) {
          super(...args);
          this.postMessage = (
            message: unknown,
            options?: Transferable[] | StructuredSerializeOptions,
          ) => {
            const packet = message as { type: string; request?: WasmRequest };
            if (packet.type === 'rpc' && packet.request)
              window.retainedRequests.push(packet.request);
            if (Array.isArray(options)) super.postMessage(message, options);
            else super.postMessage(message, options);
          };
          this.addEventListener('message', ({ data }) => {
            if (data.type === 'state') window.retainedInputs = data.state.inputs;
          });
        }
      };
    });
    const page = await context.newPage(),
      errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(url);
    await page.waitForFunction(
      () => window.cameraPerformance && window.retainedInputs.length,
      undefined,
      { timeout: 300000 },
    );
    const inputs = await page.evaluate(() => window.retainedInputs);
    assert.equal(inputs.filter((input) => input.supported).length, supported);
    assert.equal(inputs.filter((input) => !input.supported).length, unavailable);
    for (const input of inputs) {
      const control = page.locator(`[data-id="${input.id}"][aria-disabled]`);
      await control.waitFor({ state: 'visible' });
      assert.equal(await control.getAttribute('aria-disabled'), String(!input.supported), input.id);
      if (!input.supported) assert(input.unavailable, `${input.id} lost its refusal reason`);
    }
    const menu = async (active: number) =>
      page.waitForFunction(
        async (wanted) => {
          const p = await window.cameraEngine!.profile(0);
          const words = p.observations as Record<string, number[]>;
          const word = (name: string) =>
            new DataView(Uint8Array.from(words[name]!).buffer).getUint32(0, true);
          return (
            !p.failed &&
            word('ui_mode') === 3 &&
            word('menu_active') === wanted &&
            !word('pending_mode') &&
            !word('input_pending')
          );
        },
        active,
        { timeout: 120000 },
      );
    await menu(0);
    await page.screenshot({
      path: new URL(`${type.name()}-rec.png`, output).pathname,
      fullPage: true,
    });
    await page.locator('button[data-id="menu_ok_button"]').click();
    await menu(1);
    await page.screenshot({
      path: new URL(`${type.name()}-menu.png`, output).pathname,
      fullPage: true,
    });
    const undo = await page.evaluate(async () => {
      const before = await window.cameraEngine!.profile(0);
      await window.cameraEngine!.rollback(10);
      return { before, after: await window.cameraEngine!.profile(0) };
    });
    assert(BigInt(undo.after.ticks as string) < BigInt(undo.before.ticks as string));
    await menu(0);
    await page.locator('button[data-id="menu_ok_button"]').click();
    await menu(1);
    await page.locator('button[data-id="disp_back_button"]').click();
    await menu(0);
    const shutter = page.locator('[data-id="shutter_button"][role="button"]');
    const rect = await shutter.boundingBox();
    assert(rect);
    await page.mouse.move(rect.x + 10, rect.y + rect.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(500);
    await page.mouse.up();
    await page.waitForFunction(() =>
      window.retainedRequests.some((r) => r.action === 'release' && r.id === 'shutter_button'),
    );
    await menu(0);
    const requests = await page.evaluate(() =>
      window.retainedRequests.filter(
        (r) => ['press', 'release'].includes(r.action) && 'id' in r && r.id === 'shutter_button',
      ),
    );
    assert(requests.some((r) => r.action === 'press' && r.stage === 'half'));
    assert(!requests.some((r) => r.action === 'press' && r.stage === 'full'));
    const profile = await page.evaluate(() => window.cameraEngine!.profile(0));
    const jit = profile.jit as { enabled: boolean; compiledExecutions: string; failures: number };
    assert(jit.enabled && BigInt(jit.compiledExecutions) > 0n && !jit.failures);
    assert.deepEqual(await page.evaluate(() => window.retainedInputs), inputs);
    assert.deepEqual(errors, []);
    await writeFile(
      new URL(`${type.name()}.json`, output),
      JSON.stringify({ url, inputs, undo, requests, profile, errors }, null, 2) + '\n',
    );
    console.log(
      `${type.name()}: ${firmware} MENU/BACK, native Undo, physical half-press and all unavailable entries passed.`,
    );
  } finally {
    await browser.close();
  }
}
