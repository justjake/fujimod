/** Exercise release actions, shared browser firmware and both download formats without hardware. */
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile, copyFile, readdir } from 'node:fs/promises';
import { chromium, webkit } from 'playwright';
import { unzipSync } from 'fflate';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { sha256 } from '../patch/patcher.ts';
import type { Catalog } from '../catalog';
import type { PatchIndex } from '../patch/catalog';

const base = process.env.FUJIMOD_WEB_URL;
assert.ok(base, 'Set FUJIMOD_WEB_URL to the development server.');
const output = new URL('../../build/release-downloads/', import.meta.url);
assert.ok(
  process.env.FUJIMOD_REVIEW_DIR,
  'Set FUJIMOD_REVIEW_DIR to an external review directory.',
);
const review = pathToFileURL(join(process.env.FUJIMOD_REVIEW_DIR, 'release-downloads') + '/');
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
      let installable = false,
        showNotes = true,
        releaseError = false;
      const catalogResponse = await context.request.get(new URL('runtime/catalog.json', base).href);
      const catalog: Catalog = await catalogResponse.json();
      const firmware = catalog.firmwares.find((entry) => entry.id === 'x70-110');
      assert.ok(firmware);
      firmware.sha256 = hash;
      firmware.bytes = source.length;
      const entry = {
        ...firmware,
        patch_sha256: await sha256(patch),
        patch_bytes: patch.length,
        boot_instructions: ['Copy files.'],
        downloads: { sd_card: true, installable: false },
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
              ...(showNotes ? {} : { notes: undefined }),
              firmwares: [{ ...entry, downloads: { sd_card: true, installable } }],
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
      assert.equal(await page.locator('.release-hit:disabled').count(), 4);
      assert.equal(await page.locator('.release-gate').isVisible(), device === 'desktop');
      assert.equal(await page.locator('.release-actions').isVisible(), device === 'desktop');
      await screenshot('empty');
      await page.locator('#firmware-file').setInputFiles({
        name: 'FPUPDATE.DAT',
        mimeType: 'application/octet-stream',
        buffer: Buffer.from(source),
      });
      await page
        .getByRole('radio', { name: 'X70 · 1.10', exact: true })
        .waitFor({ state: 'attached' });
      assert.equal(
        await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).isChecked(),
        true,
      );
      assert.equal(
        await page
          .getByRole('button', { name: 'Download FujiMod installable', exact: true })
          .isDisabled(),
        true,
      );
      await page.evaluate(async () => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        await new CameraStore().addFirmware(new File([new Uint8Array([5])], 'another.dat'), {
          id: 'xe2s-111',
          model: 'X-E2S',
          version: '1.11',
          sha256: 'b'.repeat(64),
        });
      });
      await page.reload();
      await ready();
      // Most recently added firmware is selected, even when this browser cannot emulate it.
      assert.equal(
        await page.getByRole('radio', { name: 'X-E2S · 1.11', exact: true }).isChecked(),
        true,
      );
      await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).check();
      await screenshot('chosen');
      await page.evaluate(async (hash) => {
        const { CameraStore }: typeof import('./store') = await import(
          '/src/' + 'cameras/store.ts'
        );
        const store = new CameraStore(),
          file = new File([new Uint8Array([1, 2, 3, 4])], 'FPUPDATE.DAT');
        const identity = { id: 'x70-110', model: 'X70', version: '1.10', sha256: hash };
        await store.create(file, identity, false);
        await store.create(file, identity, true);
      }, hash);
      await page.reload();
      await ready();
      await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).check();
      assert.equal(await page.getByRole('button', { name: 'Start fresh', exact: true }).count(), 2);
      await screenshot('saved-cameras');
      await page.getByRole('button', { name: 'Download FujiMod SD boot', exact: true }).click();
      await page.locator('dialog[open] .power-combo-keys').waitFor();
      assert.equal(await page.locator('dialog .recovery-steps > li').count(), 3);
      assert.equal(await page.locator('dialog .power-combo-keys').count(), 1);
      assert.equal(
        await page
          .locator('dialog code')
          .allTextContents()
          .then((names) => names.join(',')),
        'ffsys.bin,ffboot.bin',
      );
      const disclaimer = await page.locator('.download-disclaimer').innerText();
      assert.equal(
        disclaimer,
        'No custom firmware is completely safe. FujiMod is tested in our emulator and spot-checked on real camera hardware before each release, and we believe SD card dual boot is very low risk. We still do not guarantee this software for any purpose expressed or implied.',
      );
      await screenshot('sd-modal', true);
      if (device === 'phone') {
        await page
          .getByRole('heading', { name: 'Boot from the SD card', exact: true })
          .scrollIntoViewIfNeeded();
        await page.getByRole('button', { name: 'Close', exact: true }).scrollIntoViewIfNeeded();
        await screenshot('sd-modal-bottom', true);
      }
      const sdDownload = page.waitForEvent('download');
      await page.getByRole('button', { name: 'Download SD card files', exact: true }).click();
      const sd = await sdDownload;
      assert.match(
        sd.suggestedFilename(),
        /^fujimod-modified-sd-boot-x70-1\.10-build-[a-f0-9]{8}\.zip$/,
      );
      assert.deepEqual(Object.keys(unzipSync(await readFile((await sd.path())!))).sort(), [
        'ffboot.bin',
        'ffsys.bin',
      ]);
      await page.getByRole('button', { name: 'Close', exact: true }).click();
      installable = true;
      await page.reload();
      await ready();
      await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).check();
      await screenshot('downloads-available');
      await page.getByRole('button', { name: 'Download FujiMod installable', exact: true }).click();
      await page.locator('dialog[open] .power-combo-keys').first().waitFor();
      assert.equal(await page.locator('dialog .recovery-steps > li').count(), 5);
      assert.equal(await page.locator('dialog .power-combo-keys').count(), 2);
      await screenshot('installable-modal', true);
      if (device === 'phone') {
        await page
          .getByRole('heading', { name: 'Hide the FujiMod overlay', exact: true })
          .scrollIntoViewIfNeeded();
        await screenshot('installable-modal-bottom', true);
      }
      const installDownload = page.waitForEvent('download');
      await page
        .getByRole('button', { name: 'Download FujiMod installable', exact: true })
        .last()
        .click();
      const install = await installDownload;
      assert.deepEqual(Object.keys(unzipSync(await readFile((await install.path())!))).sort(), [
        'FPUPDATE.DAT',
        'ffboot.bin',
        'ffsys.bin',
      ]);
      await page.getByRole('button', { name: 'Close', exact: true }).click();
      await page.getByRole('radio', { name: 'X-E2S · 1.11', exact: true }).check();
      await screenshot('unsupported');
      showNotes = false;
      await page.reload();
      await ready();
      assert.equal(await page.locator('.release-notes').count(), 0);
      await screenshot('no-notes');
      releaseError = true;
      await page.reload();
      await ready();
      await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).check();
      assert.match(
        await page.locator('.launcher .camera-storage-status.error').innerText(),
        /Current release unavailable/,
      );
      assert.equal(
        await page
          .getByRole('button', { name: 'Run unmodified in your browser', exact: true })
          .isEnabled(),
        true,
      );
      assert.equal(
        await page
          .getByRole('button', { name: 'Run FujiMod in your browser', exact: true })
          .isDisabled(),
        true,
      );
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
      await page.goto(base);
      await ready();
      assert.equal(await page.getByRole('radio', { name: 'X70 · 1.10', exact: true }).count(), 0);
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
      assert.deepEqual(retained, { retained: [7], cameras: 3, library: 2 });
      assert.deepEqual(errors, []);
      console.log(
        `${engine.name()} ${device}: empty, chosen, download modals and ZIPs, unsupported, no notes, shared removal and retained firmware passed`,
      );
      await context.close();
    }
  } finally {
    await browser.close();
  }
}
await writeFile(
  new URL('index.html', review),
  `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Release downloads review</title><style>body{background:#000;color:#eee;font:16px system-ui;margin:24px}a{color:#64b5ff}img{display:block;max-width:100%;height:auto;border:1px solid #333;margin:12px 0 40px}section{max-width:1280px;margin:auto}</style><section><h1>Release downloads</h1><p><a href="/#workspace">Open launcher</a></p>${(
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
