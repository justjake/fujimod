/** Exercise live layout editing and saved-camera persistence in the managed preview. */
import { chromium, type Page } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, unlink, writeFile } from 'node:fs/promises';
import type { ControlNode, DevicePanel } from '@fujimod/emulator-ui/ui/layout';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the managed web URL.');
assert.equal(base, 'https://cozycat.tail2b5ca.ts.net:8794/?scenario=x70-lcd#workspace');
const output = new URL('../../build/layout-editor/', import.meta.url);
await mkdir(output, { recursive: true });

async function ready(page: Page, timeout = 240000) {
  try {
    await page.waitForFunction(
      () =>
        document.querySelector('#live-label')?.textContent === 'LIVE' ||
        document.querySelector('#status')?.classList.contains('error') ||
        document.querySelector('dialog.firmware-actions[open]'),
      undefined,
      { timeout },
    );
  } catch (error) {
    const diagnostics = await page.evaluate(() => ({
      live: document.querySelector('#live-label')?.textContent,
      status: document.querySelector('#status')?.textContent,
      fault: document.querySelector('#fault-raw')?.textContent,
      prompt: document.querySelector('#firmware-prompt')?.textContent,
      dialogs: [...document.querySelectorAll('dialog')].map((dialog) => ({
        open: dialog.open,
        text: dialog.innerText,
      })),
    }));
    await page.screenshot({ path: new URL('boot-timeout.png', output).pathname });
    throw Error(`${String(error)} ${JSON.stringify(diagnostics)}`);
  }
  if (await page.locator('dialog.firmware-actions[open]').count())
    await page.getByRole('button', { name: 'Launch camera', exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector('#live-label')?.textContent === 'LIVE' ||
      document.querySelector('#status')?.classList.contains('error'),
    undefined,
    { timeout },
  );
  assert.equal(
    await page.locator('#live-label').innerText(),
    'LIVE',
    await page.locator('#status').innerText(),
  );
}

async function beginEditing(page: Page) {
  if (await page.locator('.edit-controls').count()) await page.locator('.edit-controls').click();
  await page.locator('.control-surface.editing-layout').waitFor();
}

/** Select a control, rebind it to another supported button and rename it through its sheet. */
async function editFirst(page: Page, caption: string) {
  await beginEditing(page);
  const control = page.locator('#controls .control-node[data-layout-path]').first();
  await control.click();
  await page.locator('.control-sheet [aria-label="Input"]').waitFor();
  const path = await control.getAttribute('data-layout-path');
  const current = await control.getAttribute('data-id');
  const bindings = await page
    .locator('.control-sheet [aria-label="Input"] option:not([disabled])')
    .evaluateAll((options) =>
      options.map((option) => (option as HTMLOptionElement).value).filter(Boolean),
    );
  const binding = bindings.find((id) => id !== current && /button/.test(id)) ?? bindings[0];
  assert.ok(path && binding, 'A live control and emulator input must be available.');
  await page.locator('.control-sheet [aria-label="Input"]').selectOption(binding);
  await page.locator('.control-sheet [aria-label="Style"]').selectOption('key');
  await page.locator('.control-sheet [aria-label="Name"]').fill(caption);
  await page.locator('.control-sheet [aria-label="Name"]').press('Enter');
  await page.locator('.control-sheet [aria-label="Name"]').blur();
  await page.waitForFunction(
    ({ path, binding, caption }) => {
      const node = document.querySelector<HTMLElement>(`[data-layout-path="${path}"]`);
      return (
        node?.dataset.id === binding &&
        node.querySelector('.cap')?.textContent === caption &&
        node.querySelector('.key') !== null
      );
    },
    { path, binding, caption },
  );
  return { path, binding };
}

async function cameraRecords(page: Page) {
  return page.evaluate(async () => {
    const { CameraStore }: typeof import('../cameras/store') = await import(
      '/src/' + 'cameras/store.ts'
    );
    return new CameraStore().list();
  });
}

function nodeAtPath(panel: DevicePanel | undefined, path: string): ControlNode | undefined {
  let nodes = panel?.layout;
  let node: ControlNode | undefined;
  for (const part of path.split('.')) {
    node = nodes?.[Number(part)];
    if (!node) return undefined;
    nodes = node.children ?? [];
  }
  return node;
}

const browser = await chromium.launch({ headless: true });
let firmwarePath: string | undefined;
try {
  const context = await browser.newContext({
    viewport: { width: 402, height: 874 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
  });
  const page = await context.newPage();
  page.setDefaultTimeout(60000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
  });

  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await ready(page);
  const edited = await editFirst(page, 'Layout editor');
  assert.notEqual(
    await page.locator('#viewer').evaluate((viewer) => getComputedStyle(viewer).position),
    'sticky',
    'The viewer must not stay pinned while editing on a phone.',
  );
  // Every control kind on the shipped X70 layout is selectable.
  for (const [selector, slot] of [
    ['#controls .dpad .a.u', 'Up'],
    ['#controls .cmd .side.l', 'Left'],
  ] as const) {
    await page.locator(selector).click();
    await page.locator(`.control-sheet [aria-label="${slot}"]`).waitFor();
  }
  await page.locator('#controls [data-id="shutter_speed_dial"]').first().click();
  await page.locator('.control-sheet [aria-label="Style"]').waitFor();
  // Tapping empty panel space dismisses the sheet, uncovering the toolbar.
  await page.locator('#controls .control-surface > hr').first().click();
  await page.locator('.control-sheet').waitFor({ state: 'detached' });
  const download = page.waitForEvent('download');
  await page.locator('[aria-label="Export layout"]').click();
  assert.equal((await download).suggestedFilename(), 'x70.layout.json');

  const imported = {
    schema: 1,
    body: 'x70',
    layout: [{ kind: 'control', id: 'unknown-layout-input', variant: 'key', caption: 'Imported' }],
    combos: [],
  };
  await page.locator('input[aria-label="Layout file"]').setInputFiles({
    name: 'x70.layout.json',
    mimeType: 'application/json',
    buffer: Buffer.from(JSON.stringify(imported)),
  });
  await page.locator('.layout-notice').getByText('unknown-layout-input').waitFor();
  await page.locator('[data-layout-path="0"]').click();
  await page.locator('.control-sheet [aria-label="Name"]').waitFor();
  await page.screenshot({ path: new URL('editor.png', output).pathname });
  console.log(`Live editor passed at ${base}; screenshot saved to ${output.pathname}editor.png`);

  const appUrl = new URL(base);
  appUrl.search = '';
  appUrl.hash = '';
  const firmwareResponse = await page.request.get(new URL('/local/firmware.dat', appUrl).href, {
    timeout: 300000,
  });
  assert.equal(firmwareResponse.ok(), true, 'The configured local X70 firmware must be available.');
  firmwarePath = new URL('FPUPDATE.DAT', output).pathname;
  await writeFile(firmwarePath, await firmwareResponse.body());
  await page.goto(appUrl.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
  const chooser = page.waitForEvent('filechooser');
  await page.locator('#firmware-prompt').click();
  await (await chooser).setFiles(firmwarePath);
  await ready(page, 60000);
  const initialCameras = await cameraRecords(page);
  assert.equal(initialCameras.length, 1);
  const savedEdit = await editFirst(page, 'Saved layout');
  await page.waitForFunction(
    async ({ path, caption }) => {
      const { CameraStore }: typeof import('../cameras/store') = await import(
        '/src/' + 'cameras/store.ts'
      );
      const records = await new CameraStore().list();
      let nodes = records[0]?.layout?.layout;
      let node: ControlNode | undefined;
      for (const part of path.split('.')) {
        node = nodes?.[Number(part)];
        if (!node) return false;
        nodes = node.children;
      }
      return node?.caption === caption;
    },
    { ...savedEdit, caption: 'Saved layout' },
  );
  await page.goto(appUrl.href, { waitUntil: 'domcontentloaded' });
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
  await page.locator('.resume-camera').first().waitFor();
  const afterReload = await cameraRecords(page);
  assert.equal(nodeAtPath(afterReload[0]?.layout, savedEdit.path)?.caption, 'Saved layout');
  await page.locator('.resume-camera').first().click();
  await ready(page);
  await page.waitForFunction(
    ({ binding, caption }) =>
      [...document.querySelectorAll<HTMLElement>(`#controls .control-node[data-id="${binding}"]`)].some(
        (node) => node.querySelector('.cap')?.textContent === caption,
      ),
    { ...savedEdit, caption: 'Saved layout' },
  );
  assert.deepEqual(errors, []);
  await writeFile(
    new URL('result.json', output),
    JSON.stringify({ url: base, edited, savedEdit, errors }, null, 2),
  );
  console.log(
    'Passed: import/export, unknown IDs, live rendering, and saved-camera layout restore',
  );
} finally {
  await browser.close();
  if (firmwarePath) await unlink(firmwarePath).catch(() => {});
}
