import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium, webkit, devices } from 'playwright';
import { unzipSync, strFromU8 } from 'fflate';
import catalog from '../src/home/features.generated.json' with { type: 'json' };
import { loadRelease } from '../src/patch/release.ts';
import { loadOtaRelease, sdBootV2Firmware, storeName } from '../src/ota/release.ts';

const base = process.env.FUJIMOD_WEB_URL || 'http://127.0.0.1:5201/';
const output = resolve(process.env.FUJIMOD_REVIEW_OUTPUT || 'build/release-showcase');
const release = await loadRelease(base);
assert.equal(release?.release, '0.1.0', 'run against the 0.1.0 preview');
assert.ok(release);
await mkdir(output, { recursive: true });
const digest = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch();
  try {
    for (const size of ['desktop', 'mobile', 'narrow'] as const) {
      const context = await browser.newContext(
        size === 'desktop'
          ? { viewport: { width: 1280, height: 1000 } }
          : {
              ...devices['iPhone 13'],
              viewport: { width: size === 'mobile' ? 390 : 320, height: 844 },
            },
      );
      try {
        const page = await context.newPage();
        const errors: string[] = [];
        page.on('pageerror', (error) => errors.push(error.message));
        page.on('console', (message) => {
          if (/Hydration (?:structure|key|tag)|REACTIVITY_HALTED/.test(message.text()))
            errors.push(message.text());
        });
        assert.equal((await page.goto(base))?.status(), 200);
        await page.locator('.sd-camera').first().waitFor();
        assert.equal(await page.locator('.catalog-tile').count(), catalog.features.length);
        assert.equal(await page.locator('.release-highlight').count(), 6);
        assert.equal(await page.locator('#camera-table').count(), 0);
        assert.equal(await page.locator('.sd-camera').count(), 3);
        assert.match(await page.locator('.sd-release-version').innerText(), /0\.1\.0.*Preview/);
        await page.locator('.release-showcase').screenshot({
          path: resolve(output, `${size}-${engine.name()}.png`),
        });
        assert.ok(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
          `${engine.name()} ${size}: no horizontal overflow`,
        );

        await page.locator('.release-highlight').nth(1).click();
        const search = page.getByRole('searchbox', { name: 'Find a feature' });
        await page.waitForFunction(
          () => document.querySelector<HTMLInputElement>('.catalog-search input')?.value === 'film',
        );
        assert.ok((await page.locator('.catalog-tile').count()) < catalog.features.length);
        await page.locator('.catalog-ref').first().focus();
        await page.locator('.catalog-card.on').waitFor();
        await page.keyboard.press('Escape');
        await search.fill('no-such-feature');
        await page.getByRole('status').filter({ hasText: 'No matching features.' }).waitFor();
        await search.fill('');
        assert.equal(await page.locator('.catalog-tile').count(), catalog.features.length);
        await page.getByRole('link', { name: 'Get started', exact: true }).click();
        assert.equal(new URL(page.url()).hash, '#quickstart');
        await page.locator('#release-notes summary').click();
        assert.match(await page.locator('#release-notes').innerText(), /Known issues/);
        await page.locator('#release-notes summary').click();

        const cameras = page.locator('.sd-camera input[type="checkbox"]');
        await cameras.nth(0).check();
        const download = page.locator('.sd-download-button');
        await page.waitForFunction(
          () => !document.querySelector<HTMLButtonElement>('.sd-download-button')?.disabled,
        );
        await page.locator('.power-combo-keys').first().waitFor();
        assert.equal(await page.locator('.sd-problem').count(), 0);
        if (engine === chromium && size === 'desktop') {
          await cameras.nth(1).check();
          await cameras.nth(2).check();
          await cameras.nth(1).uncheck();
          await cameras.nth(1).check();
          await page.waitForFunction(
            () => !document.querySelector<HTMLButtonElement>('.sd-download-button')?.disabled,
          );
          await page.locator('.sd-boot-instructions .power-combo-keys').nth(2).waitFor();
          assert.equal(await page.locator('.sd-boot-instructions [role="alert"]').count(), 0);
          const downloaded = page.waitForEvent('download');
          await download.click();
          const file = await downloaded;
          assert.equal(file.suggestedFilename(), 'fujimod-sd-card-0.1.0.zip');
          const path = await file.path();
          assert.ok(path);
          const files = unzipSync(new Uint8Array(await readFile(path)));
          for (const asset of release.sd_card_files ?? []) {
            assert.ok(files[asset.path], `download includes ${asset.path}`);
            assert.equal(digest(files[asset.path]), asset.sha256);
          }
          for (const camera of release.firmwares) {
            const firmware = sdBootV2Firmware(camera.id);
            const manifest = await loadOtaRelease(release.assetBase, firmware);
            const root = `FUJIMOD/SYSTEM/${firmware}`;
            const initial = JSON.parse(strFromU8(files[`${root}/A.JSN`]));
            assert.equal(initial.version, manifest.version);
            assert.deepEqual(
              initial.fs,
              Object.fromEntries(
                Object.entries(manifest.fs).map(([path, file]) => [path, file.sha256]),
              ),
            );
            assert.ok(files[`${root}/STATE.JSN`]);
            for (const file of Object.values(manifest.fs))
              assert.equal(
                digest(files[`FUJIMOD/SYSTEM/STORE/${storeName(file.sha256)}`]),
                file.sha256,
              );
          }
          console.log(
            `SD card download: ${Object.keys(files).length} files, all release hashes verified`,
          );
        }
        await page.locator('.sd-download').screenshot({
          path: resolve(output, `${size}-${engine.name()}-sd-card.png`),
        });
        assert.ok(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
          `${engine.name()} ${size}: selected SD card fits`,
        );
        assert.deepEqual(errors, [], `${engine.name()} ${size}: no hydration or page errors`);
        console.log(
          `${engine.name()} ${size}: showcase, catalog, notes and SD card selection passed`,
        );
      } finally {
        await context.close();
      }
    }
  } finally {
    await browser.close();
  }
}
