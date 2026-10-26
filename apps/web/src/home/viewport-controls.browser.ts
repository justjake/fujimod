/** LCD-only walkthrough against real firmware in the configured development preview. */
import { chromium, webkit, type Page } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
const dat = process.env.FUJIMOD_TEST_DAT;
if (!base || !dat) throw Error('Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/viewport-controls/', import.meta.url);
await mkdir(output, { recursive: true });

async function screenshot(page: Page, name: string) {
  await page.screenshot({ path: new URL(`${name}.png`, output).pathname, fullPage: true });
}
async function ready(page: Page) {
  await page.waitForFunction(
    () =>
      document.querySelector('#live-label')?.textContent === 'LIVE' ||
      document.querySelector('#status')?.classList.contains('error'),
    undefined,
    { timeout: 240000 },
  );
  assert.equal(
    await page.locator('#live-label').innerText(),
    'LIVE',
    await page.locator('#status').innerText(),
  );
  await page.waitForFunction(
    () => !document.querySelector<HTMLButtonElement>('#firmware-mode button')?.disabled,
  );
}
async function menu(page: Page) {
  await page.locator('#firmware-mode > button').click();
  assert.equal(await page.locator('#firmware-mode > button').getAttribute('aria-expanded'), 'true');
}
async function closeMenu(page: Page) {
  await page.locator('#firmware-mode > button').press('Escape');
}
const browser = await chromium.launch({ headless: true });
let activePage: Page | undefined;
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  // Pin each navigation to its loaded source while other development work continues.
  await context.routeWebSocket('**/*', () => {});
  const page = await context.newPage(),
    errors: string[] = [],
    graphicsRequests: string[] = [];
  page.on('request', (request) => {
    if (/simulation[\/-]|\.glb(?:\?|$)|\/environments\//.test(request.url()))
      graphicsRequests.push(request.url());
  });
  activePage = page;
  page.setDefaultTimeout(60000);
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
  });
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  assert.equal(await page.locator('#camera-exit').count(), 0);
  assert.equal(await page.locator('#idle-cameras').count(), 0);
  await page.locator('#workspace').scrollIntoViewIfNeeded();
  await screenshot(page, 'first-use');
  assert.equal(await page.locator('#viewer-tools').isVisible(), false);
  await page.goto(new URL('?feedback', base).href, { waitUntil: 'domcontentloaded' });
  await page.locator('#feedback-dialog[open]').waitFor();
  await page.locator('#feedback-description').fill('Viewport walkthrough');
  assert.equal(
    await page.locator('#feedback-dialog').evaluate((e) => (e as HTMLDialogElement).open),
    true,
  );
  await page.locator('#feedback-dialog button[value="cancel"]').click();
  const chooser = page.waitForEvent('filechooser');
  await page.locator('#firmware-prompt').click();
  await (await chooser).setFiles(dat);
  await ready(page);
  console.log('Created first camera from native file chooser and booted firmware');
  assert.equal(await page.locator('#camera-exit').isVisible(), true);
  assert.equal(
    await page.locator('#lcd-source').evaluate((canvas: HTMLCanvasElement) =>
      canvas
        .getContext('2d')!
        .getImageData(0, 0, canvas.width, canvas.height)
        .data.some((value) => value > 0),
    ),
    true,
  );
  for (const width of [320, 390, 820, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.locator('#viewer-tools').scrollIntoViewIfNeeded();
    await menu(page);
    const bounds = await page.locator('#viewer-tools,.firmware-menu').evaluateAll((elements) =>
      elements.map((element) => {
        const rect = element.getBoundingClientRect();
        return { left: rect.left, right: rect.right, top: rect.top };
      }),
    );
    assert.ok(
      bounds.every((rect) => rect.left >= -1 && rect.right <= width + 1 && rect.top >= 0),
      JSON.stringify({ width, bounds }),
    );
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
      false,
      JSON.stringify(
        await page.evaluate(() => ({
          width: innerWidth,
          scroll: document.documentElement.scrollWidth,
          outside: [...document.querySelectorAll('*')]
            .map((element) => ({
              tag: element.tagName,
              id: element.id,
              class: element.getAttribute('class'),
              right: element.getBoundingClientRect().right,
            }))
            .filter((item) => item.right > innerWidth + 1)
            .slice(0, 25),
        })),
      ),
    );
    await screenshot(page, `layout-${width}`);
    await closeMenu(page);
  }
  console.log('Responsive viewport geometry passed');
  await menu(page);
  await screenshot(page, 'settings');
  await closeMenu(page);
  assert.equal(await page.locator('.firmware-menu').count(), 0);
  await menu(page);
  await page.locator('#emulator-heading').click();
  assert.equal(await page.locator('.firmware-menu').count(), 0);
  for (const label of ['Unmodded', 'FujiMod']) {
    await menu(page);
    await page.locator(`[data-mod-enabled="${label === 'FujiMod'}"]`).click();
    await ready(page);
    console.log(`${label} boot passed`);
    assert.equal(await page.locator('#firmware-mode > button').innerText(), label);
    await menu(page);
    assert.equal(
      await page
        .locator(`[data-mod-enabled="${label === 'FujiMod'}"]`)
        .getAttribute('aria-checked'),
      'true',
    );
    await screenshot(page, label.toLowerCase());
    await closeMenu(page);
  }
  // The shareable preview executes the same checked native boot from a fixed initial state.
  const preview = new URL('?scenario=x70-lcd#workspace', base).href;
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(preview, { waitUntil: 'domcontentloaded' });
  await ready(page);
  assert.equal(await page.locator('#viewer').getAttribute('data-renderer'), 'lcd');
  assert.equal(
    await page.locator('#simulation-canvas,#view-3d,#view-lcd,#face-screen,#face-lens').count(),
    0,
  );
  assert.equal(await page.getByRole('group', { name: '3D renderer' }).count(), 0);
  const frames = Number(await page.locator('#viewer').getAttribute('data-frames'));
  assert.ok(frames > 0, 'The checked boot must present a real LCD image.');
  const beforeMenu = await page
    .locator('#lcd-source')
    .evaluate((canvas: HTMLCanvasElement) => canvas.toDataURL());
  await page.locator('#controls button[data-id="menu_ok_button"]').click();
  await page.waitForFunction(
    ({ frames, pixels }) =>
      Number(document.querySelector<HTMLElement>('#viewer')?.dataset.frames) > frames &&
      document.querySelector<HTMLCanvasElement>('#lcd-source')!.toDataURL() !== pixels,
    { frames, pixels: beforeMenu },
  );
  await screenshot(page, 'shared-preview');
  assert.deepEqual(graphicsRequests, []);

  assert.deepEqual(errors, []);
  await writeFile(
    new URL('result.json', output),
    JSON.stringify(
      {
        preview,
        errors,
        controls: ['FujiMod', 'Unmodded', 'Feedback'],
        widths: [320, 390, 820, 1440],
      },
      null,
      2,
    ),
  );
  console.log(
    'Passed: first upload, real native boots, LCD rendering, both firmware modes, feedback, menu dismissal, responsive layouts and shared preview',
  );
} catch (error) {
  if (activePage) await screenshot(activePage, 'failure').catch(() => {});
  console.error(error);
  throw error;
} finally {
  await browser.close();
}

const safari = await webkit.launch();
try {
  const page = await safari.newPage({ viewport: { width: 390, height: 844 } });
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  assert.equal(await page.locator('#camera-exit').count(), 0);
  assert.equal(await page.locator('#viewer-tools').isVisible(), false);
  await page.goto(new URL('?feedback', base).href, { waitUntil: 'domcontentloaded' });
  await page.locator('#feedback-dialog[open]').waitFor();
  assert.equal(
    await page.locator('#feedback-dialog').evaluate((e) => (e as HTMLDialogElement).open),
    true,
  );
  await screenshot(page, 'webkit-feedback');
  await page.locator('#feedback-dialog button[value="cancel"]').click();
  const graphicsRequests: string[] = [];
  page.on('request', (request) => {
    if (/simulation[\/-]|\.glb(?:\?|$)|\/environments\//.test(request.url()))
      graphicsRequests.push(request.url());
  });
  await page.goto(new URL('?scenario=x70-lcd#workspace', base).href, {
    waitUntil: 'domcontentloaded',
  });
  await ready(page);
  assert.equal(await page.locator('#viewer').getAttribute('data-renderer'), 'lcd');
  assert.equal(await page.locator('#simulation-canvas,#view-3d').count(), 0);
  await screenshot(page, 'webkit-lcd');
  assert.deepEqual(graphicsRequests, []);
  console.log(
    'Passed: WebKit first use without idle menu, feedback and real LCD boot without 3D assets',
  );
} finally {
  await safari.close();
}
