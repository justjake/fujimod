import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium } from 'playwright';
import assert from 'node:assert/strict';

const base = process.env.FUJIMOD_WEB_URL;
const nativeWebSocket = process.env.FUJIMOD_NATIVE_WS_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const screenshots = resolve(
  process.env.FUJIMOD_NATIVE_SCREENSHOTS ?? 'out/glgui-native',
);
await mkdir(screenshots, { recursive: true });

const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    deviceScaleFactor: 1,
  });
  const page = await context.newPage();
  page.setDefaultTimeout(20000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const url = new URL('/review/glgui-native/', base);
  if (nativeWebSocket) url.searchParams.set('ws', nativeWebSocket);
  await page.goto(url.href, { waitUntil: 'domcontentloaded' });
  const loadFinished = Date.now();
  await page.waitForFunction(
    () =>
      document.querySelector('#status')?.getAttribute('data-state') === 'LIVE' &&
      (window as Window & { nativeFrameCount?: number }).nativeFrameCount! >= 1 &&
      (window as Window & { nativeDisplayState?: { source: string } }).nativeDisplayState?.source ===
        'stock',
    undefined,
    { timeout: 3000 },
  );
  await new Promise<void>((done) =>
    setTimeout(done, Math.max(0, 3000 - (Date.now() - loadFinished))),
  );

  const canvas = page.locator('#lcd');
  assert.equal(await canvas.getAttribute('width'), '720');
  assert.equal(await canvas.getAttribute('height'), '480');
  const visibleControls = [
    page.getByRole('button', { name: 'MENU', exact: true }),
    page.getByRole('button', { name: 'BACK', exact: true }),
    page.getByRole('button', { name: 'Q', exact: true }),
    page.getByRole('button', { name: 'OK', exact: true }).first(),
    page.getByRole('button', { name: 'Up', exact: true }),
    page.getByRole('button', { name: 'Down', exact: true }),
    page.getByRole('button', { name: 'Left', exact: true }),
    page.getByRole('button', { name: 'Right', exact: true }),
    page.getByRole('button', { name: 'Fn', exact: true }),
    page.getByRole('button', { name: 'Command dial left', exact: true }),
    page.getByRole('button', { name: 'Command dial right', exact: true }),
    page.getByRole('button', { name: 'Exposure compensation down', exact: true }),
    page.getByRole('button', { name: 'Exposure compensation up' }),
    page.getByRole('button', { name: 'Shutter speed down', exact: true }),
    page.getByRole('button', { name: 'Shutter speed up', exact: true }),
  ];
  for (const control of visibleControls) assert.equal(await control.isVisible(), true);
  const colors = await page.evaluate(() => {
    const lcd = document.querySelector<HTMLCanvasElement>('#lcd');
    const context = lcd?.getContext('2d');
    if (!lcd || !context) return 0;
    const pixels = context.getImageData(0, 0, lcd.width, lcd.height).data;
    const sampled = new Set<string>();
    for (let index = 0; index < pixels.length; index += 384) {
      sampled.add(`${pixels[index]},${pixels[index + 1]},${pixels[index + 2]}`);
    }
    return sampled.size;
  });
  assert.ok(colors > 20, `Expected a live scene frame with varied pixels; saw ${colors} colors.`);
  await page.screenshot({ path: resolve(screenshots, 'phone-live-3s.png') });

  await canvas.click({ position: { x: 360, y: 240 } });
  await page.waitForFunction(
    () => (window as Window & { nativeTouchCount?: number }).nativeTouchCount! >= 2,
    undefined,
    { timeout: 10000 },
  );
  assert.match((await canvas.getAttribute('data-last-touch')) ?? '', /"accepted":true/);

  await page.getByRole('button', { name: 'MENU', exact: true }).click();
  await page.waitForFunction(
    () => {
      const state = (window as Window & {
        nativeDisplayState?: { source: string; title: string | null };
      }).nativeDisplayState;
      return state?.source === 'glgui' && state.title === 'SHOOTING MENU';
    },
    undefined,
    { timeout: 32000 },
  );
  assert.equal(await page.locator('#status').textContent(), 'SHOOTING MENU');
  await page.screenshot({ path: resolve(screenshots, 'phone-menu.png') });

  await page.evaluate(async () => {
    const engine = (
      window as Window & {
        nativeEngine?: {
          press(id: string): Promise<void>;
          release(id: string): Promise<void>;
          rotate(id: string, steps: number): Promise<void>;
          dispose(): Promise<void>;
        };
      }
    ).nativeEngine;
    if (!engine) throw Error('Native engine was not installed on the review page.');
    await engine.press('down');
    await engine.release('down');
    await engine.rotate('shutter_speed_dial', 1);
    await engine.dispose();
  });
  assert.deepEqual(errors, []);

  const scenarioPage = await context.newPage();
  const scenarioErrors: string[] = [];
  scenarioPage.on('pageerror', (error) => scenarioErrors.push(error.message));
  const scenarioUrl = new URL('/review/glgui-native/', base);
  scenarioUrl.searchParams.set('scenario', 'shooting-menu');
  if (nativeWebSocket) scenarioUrl.searchParams.set('ws', nativeWebSocket);
  await scenarioPage.goto(scenarioUrl.href, { waitUntil: 'domcontentloaded' });
  await scenarioPage.waitForFunction(
    () => {
      const state = (window as Window & {
        nativeDisplayState?: { source: string; title: string | null };
      }).nativeDisplayState;
      return state?.source === 'glgui' && state.title === 'SHOOTING MENU';
    },
    undefined,
    { timeout: 32000 },
  );
  assert.equal(await scenarioPage.locator('#status').textContent(), 'SHOOTING MENU');
  await scenarioPage.screenshot({ path: resolve(screenshots, 'phone-scenario-menu.png') });
  const scenarioCanvas = scenarioPage.locator('#lcd');
  await scenarioCanvas.click({ position: { x: 360, y: 240 } });
  await scenarioPage.waitForFunction(
    () => (window as Window & { nativeTouchCount?: number }).nativeTouchCount! >= 2,
    undefined,
    { timeout: 10000 },
  );
  assert.match((await scenarioCanvas.getAttribute('data-last-touch')) ?? '', /"accepted":true/);
  await scenarioPage.evaluate(async () => {
    const engine = (window as Window & { nativeEngine?: { dispose(): Promise<void> } })
      .nativeEngine;
    if (!engine) throw Error('Native engine was not installed on the scenario URL.');
    await engine.dispose();
  });
  assert.deepEqual(scenarioErrors, []);
  await context.close();
} finally {
  await browser.close();
}
console.log(`Native live REC frame, LCD touch, GLGUI menu and physical controls passed. Screenshots: ${screenshots}`);
