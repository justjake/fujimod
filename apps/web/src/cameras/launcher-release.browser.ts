/** Exercise the launcher's release states and shared browser firmware without hardware. */
import assert from 'node:assert/strict';
import { mkdir, writeFile, copyFile, readdir } from 'node:fs/promises';
import { chromium, webkit } from 'playwright';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { sha256 } from '../patch/patcher.ts';
import type { Catalog } from '../catalog';
import type { PatchIndex } from '../patch/catalog';

const base = process.env.FUJIMOD_WEB_URL;
assert.ok(base, 'Set FUJIMOD_WEB_URL to the development server.');
const output = new URL('../../build/launcher-release/', import.meta.url);
assert.ok(
  process.env.FUJIMOD_REVIEW_DIR,
  'Set FUJIMOD_REVIEW_DIR to an external review directory.',
);
const review = pathToFileURL(join(process.env.FUJIMOD_REVIEW_DIR, 'launcher-release') + '/');
await mkdir(output, { recursive: true });
await mkdir(review, { recursive: true });
const source = new Uint8Array([1, 2, 3, 4]);
const hash = await sha256(source);
const notes = new TextEncoder().encode(`# What's new
- **Faster menus.** Film simulation scrolling is about 4× cheaper; themed menus no longer redraw unchanged text.
- **HIGHLIGHT** exposure now previews in live view, not only after a half-press.
- F-Log movies are tagged as F-Log in their metadata.
- Touch no longer stops responding after using the tab bar with the D-pad.

## Please test
- MULTI vs HIGHLIGHT on a bright scene, compared with stock firmware.
- The CUPERTINO Q menu: tiles should stay glass, never turn opaque.
- Dev Tools → OVERLAY RENDERING while scrolling film simulations.

## Known issues
- Walkthrough coverage is incomplete; report anything odd from the bug report screen.
- CUPERTINO and X100VI menus cost more to draw than DEFAULT.`);
const program = new Uint8Array(63);
for (let target = 0; target < 3; target++) {
  const offset = target * 21;
  program.set([3, target], offset);
  new DataView(program.buffer).setUint32(offset + 8, 1, true);
  program[offset + 20] = target + 1;
}
const identities = await Promise.all(
  [1, 2, 3].map(async (value) => ({ bytes: 1, sha256: await sha256(new Uint8Array([value])) })),
);
const metadata = new TextEncoder().encode(
  JSON.stringify({
    schema: 1,
    firmware: 'x70-110',
    release: '0.0.1',
    source: { bytes: source.length, sha256: hash },
    outputs: { ffboot: identities[0], ffsys: identities[1], fpupdate: identities[2] },
    commands: 3,
    program_sha256: await sha256(program),
    mods: ['example'],
  }),
);
const patch = new Uint8Array(12 + metadata.length + program.length);
patch.set(new TextEncoder().encode('FJMODP01'));
new DataView(patch.buffer).setUint32(8, metadata.length, true);
patch.set(metadata, 12);
patch.set(program, 12 + metadata.length);
const errors: string[] = [];
for (const engine of [chromium, webkit].filter(
  (engine) =>
    !process.env.FUJIMOD_TEST_BROWSER || process.env.FUJIMOD_TEST_BROWSER === engine.name(),
)) {
  const browser = await engine.launch();
  try {
    for (const device of ['desktop', 'phone'] as const) {
      const context = await browser.newContext({
        viewport:
          device === 'desktop' ? { width: 1280, height: 1100 } : { width: 390, height: 844 },
        acceptDownloads: true,
      });
      await context.routeWebSocket('**/*', () => {});
      let releaseError = false;
      const catalogResponse = await context.request.get(new URL('runtime/catalog.json', base).href);
      const catalog: Catalog = await catalogResponse.json();
      const firmware = catalog.firmwares.find((entry) => entry.id === 'x70-110');
      assert.ok(firmware);
      firmware.sha256 = hash;
      firmware.bytes = source.length;
      const entry = {
        ...firmware,
        release_date: '2016-01-21',
        patch_sha256: await sha256(patch),
        patch_bytes: patch.length,
        boot_instructions: ['Copy files.'],
        update_filename: 'FPUPDATE.DAT',
        mods: [],
        downloads: { sd_boot_v1: true, sd_boot_v1_installable: false },
      };
      const index: PatchIndex = {
        schema: 2,
        release: '0.0.1',
        patches: { [hash]: 'example.fmpatch' },
        notes: { name: 'notes.md' },
        firmwares: [entry],
      };
      await context.route('**/runtime/catalog.json', (route) => route.fulfill({ json: catalog }));
      await context.route('**/releases/**', (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path === '/releases/current' && releaseError) return route.fulfill({ status: 500 });
        if (path === '/releases/current')
          return route.fulfill({
            json: {
              tag: '0.0.1',
              name: 'FujiMod 0.0.1',
              published_at: '2026-10-26T00:00:00Z',
              prerelease: true,
            },
          });
        if (path.endsWith('/patches.json'))
          return route.fulfill({
            json: {
              ...index,
              firmwares: [
                { ...entry, downloads: { sd_boot_v1: true, sd_boot_v1_installable: false } },
              ],
            },
          });
        if (path.endsWith('/notes.md'))
          return route.fulfill({ contentType: 'text/markdown', body: Buffer.from(notes) });
        if (path.endsWith('/example.fmpatch')) return route.fulfill({ body: Buffer.from(patch) });
        return route.fulfill({ status: 404 });
      });
      const page = await context.newPage();
      page.setDefaultTimeout(60000);
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (/\[(STRICT_|REACTIVE_|LOADING_|HOT_SCOPE_)/.test(message.text()))
          errors.push(message.text());
      });
      async function ready() {
        await page.waitForFunction(
          () => performance.getEntriesByName('camera-catalog-ready').length > 0,
        );
        await page.locator('.launcher:not(.launcher-pending)').waitFor();
      }
      async function screenshot(state: string, modal = false) {
        assert.equal(
          await page
            .locator('.launcher')
            .evaluate((element) => element.scrollWidth > element.clientWidth),
          false,
          `${device} launcher must not overflow`,
        );
        if (!modal) await page.locator('#software').scrollIntoViewIfNeeded();
        const name = `${engine.name()}-${device}-${state}.png`;
        if (modal) await page.screenshot({ path: new URL(name, output).pathname });
        else {
          const heading = (await page.locator('.emulator-heading').boundingBox())!;
          const stage = (await page.locator('#workspace').boundingBox())!;
          await page.screenshot({
            path: new URL(name, output).pathname,
            fullPage: true,
            clip: {
              x: 0,
              y: Math.max(0, heading.y + (await page.evaluate(() => scrollY)) - 12),
              width: device === 'desktop' ? 1280 : 390,
              height: heading.height + stage.height + 120,
            },
          });
        }
        await copyFile(new URL(name, output), new URL(name, review));
      }
      await page.goto(base);
      await ready();
      assert.equal(await page.locator('.launch-fujimod').isEnabled(), true);
      await screenshot('ready');
      await page.evaluate(async (hash) => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        await new CameraStore().addFirmware(new File([new Uint8Array([5])], 'another.dat'), {
          id: 'xe2s-111',
          model: 'X-E2S',
          version: '1.11',
          sha256: 'b'.repeat(64),
        });
        await new CameraStore().addFirmware(
          new File([new Uint8Array([1, 2, 3, 4])], 'FPUPDATE.DAT'),
          {
            id: 'x70-110',
            model: 'X70',
            version: '1.10',
            sha256: hash,
          },
        );
      }, hash);
      releaseError = true;
      await page.reload();
      await ready();
      assert.match(await page.locator('.launch-error').innerText(), /Current release unavailable/);
      assert.equal(await page.locator('.launch-fujimod').isDisabled(), true);
      assert.equal(await page.locator('#fault-heading').isVisible(), false);
      await screenshot('release-error');
      await page.goto(new URL('recovery/', base).href);
      await page
        .getByRole('radio', { name: 'X70 · 1.10', exact: true })
        .waitFor({ state: 'attached' });
      await page.getByRole('button', { name: 'Remove X70 · 1.10', exact: true }).click();
      await page
        .getByRole('radio', { name: 'X70 · 1.10', exact: true })
        .waitFor({ state: 'detached' });
      assert.equal(
        await page.getByRole('radio', { name: 'X-E2S · 1.11', exact: true }).isChecked(),
        true,
      );
      // Firmware used by an owned camera survives removal and can be added back.
      const retained = await page.evaluate(async () => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        const store = new CameraStore(),
          file = new File([new Uint8Array([7])], 'saved.dat');
        const identity = { id: 'x70-110', model: 'X70', version: '1.10', sha256: 'c'.repeat(64) };
        await store.create(file, identity, false);
        await store.removeFirmware(identity.sha256);
        const retained = [...new Uint8Array(await store.firmwareBytes(identity.sha256))];
        await store.addFirmware(file, identity);
        return {
          retained,
          cameras: (await store.list()).length,
          library: (await store.library()).length,
        };
      });
      assert.deepEqual(retained, { retained: [7], cameras: 1, library: 2 });
      assert.deepEqual(errors, []);
      console.log(
        `${engine.name()} ${device}: ready, release error, shared removal and retained firmware passed`,
      );
      await context.close();
    }
  } finally {
    await browser.close();
  }
}
await writeFile(
  new URL('index.html', review),
  `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Launcher release review</title><style>body{background:#000;color:#eee;font:16px system-ui;margin:24px}a{color:#64b5ff}img{display:block;max-width:100%;height:auto;border:1px solid #333;margin:12px 0 40px}section{max-width:1280px;margin:auto}</style><section><h1>Launcher release states</h1><p><a href="/#workspace">Open launcher</a></p>${(
    await readdir(review)
  )
    .filter((name) => name.endsWith('.png'))
    .sort()
    .map(
      (name) =>
        `<h2>${name.replace('.png', '')}</h2><a href="${name}"><img src="${name}" loading="lazy" alt="${name}"></a>`,
    )
    .join('')}</section></html>`,
);
console.log(`Screenshots: ${output.pathname}`);
