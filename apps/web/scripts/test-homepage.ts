import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium, webkit, devices } from 'playwright';
import { unzipSync, strFromU8 } from 'fflate';
import { manualPageAt } from '../src/docs/manual-data.ts';
import { loadRelease } from '../src/patch/release.ts';
import { loadOtaRelease, sdBootV2Firmware, storeName } from '../src/ota/release.ts';
import type { FeatureMedia } from '../src/home/feature-media.ts';

const base = process.env.FUJIMOD_WEB_URL || 'http://127.0.0.1:5201/';
const output = resolve(process.env.FUJIMOD_REVIEW_OUTPUT || 'build/homepage');
const release = await loadRelease(base);
assert.ok(release, 'Use a preview with a release loaded');
const recipes =
  release.sd_card_files?.filter((file) => file.path.startsWith('FUJIMOD/RECIPES/')) ?? [];
assert.ok(recipes.length >= 2, 'The preview release should carry recipes');
const response = await fetch(new URL('artifacts/docs/media.json', base));
const media = (response.ok ? await response.json() : { cards: {} }) as FeatureMedia;
const reels = Object.entries(media.reels ?? {});
const digest = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
await mkdir(output, { recursive: true });

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch();
  try {
    for (const mobile of [false, true]) {
      const page = await browser.newPage({
        ...(mobile ? devices['iPhone 15'] : { viewport: { width: 1280, height: 1000 } }),
        deviceScaleFactor: 2,
      });
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (
          /Hydration (?:structure|key|tag)|\[(?:REACTIVITY_HALTED|STRICT_READ_UNTRACKED)\]/.test(
            message.text(),
          )
        )
          errors.push(message.text());
      });
      assert.equal((await page.goto(base))?.status(), 200);
      await page.locator('.sd-camera').first().waitFor();
      const sections = await page.locator('main').evaluate((main) => {
        const selectors = [
          '.release-personal-note',
          '.home-reels',
          '.home-release',
          '#workspace',
          '#camera-table',
        ];
        return selectors.map((selector) =>
          Math.round(main.querySelector(selector)!.getBoundingClientRect().top),
        );
      });
      assert.deepEqual(
        sections,
        [...sections].sort((a, b) => a - b),
      );
      assert.equal(await page.locator('.catalog-tile,.sd-tree,#features').count(), 0);
      assert.deepEqual(
        await page.locator('.home-reel').evaluateAll((figures) => figures.map(({ id }) => id)),
        ['reel-shooting', 'reel-looks', 'reel-controls'],
      );
      assert.equal(await page.locator('#controls').count(), 1);
      const sectionGeometry = await page.locator('.home-release-notes').boundingBox();
      const cardGeometry = await page.locator('.home-download').boundingBox();
      assert.ok(sectionGeometry && cardGeometry);
      if (mobile) assert.ok(cardGeometry.y < sectionGeometry.y);
      else assert.ok(cardGeometry.x > sectionGeometry.x);
      assert.equal(
        await page.locator('.release-personal-note time').getAttribute('datetime'),
        await page.locator('.home-release-title time').getAttribute('datetime'),
      );
      assert.ok(
        await page
          .locator('.home-release-notes .release-notes')
          .evaluate((notes) => notes.scrollHeight <= notes.clientHeight),
        'Release notes use the page scroll',
      );
      assert.match(await page.locator('.home-sd-card').innerText(), /lives on your SD card/);
      assert.equal(await page.locator('.home-release-notes a[href="/releases"]').count(), 1);
      const releaseHeading = page.locator('.home-release-title');
      const title = await releaseHeading.evaluate((heading) =>
        heading.firstChild?.textContent?.trim(),
      );
      assert.ok(title);
      assert.equal(
        await page
          .locator('.home-release-notes .release-notes h4')
          .getByText(title, { exact: true })
          .count(),
        0,
      );
      assert.equal(
        await page.locator('#workspace .release-notes, #workspace .release-heading').count(),
        0,
      );
      await page.locator('#workspace .launch-fujimod').waitFor({ state: 'attached' });
      for (const href of await page
        .locator('main .manual-feature-link')
        .evaluateAll((links) => links.map((link) => link.getAttribute('href')!))) {
        const url = new URL(href, base);
        assert.ok(
          manualPageAt(url.pathname) &&
            (!url.hash ||
              manualPageAt(url.pathname)?.sections.some(({ anchor }) => '#' + anchor === url.hash)),
          href,
        );
      }
      const reelVideos = page.locator('.home-reel video');
      assert.ok(
        await reelVideos.evaluateAll((videos: HTMLVideoElement[]) =>
          videos.every(
            (video) =>
              !video.controls &&
              video.autoplay &&
              video.playsInline &&
              (video.muted || video.defaultMuted),
          ),
        ),
      );
      assert.equal(await reelVideos.count(), reels.length);
      if (process.env.FUJIMOD_EXPECT_REELS)
        assert.equal(reels.length, Number(process.env.FUJIMOD_EXPECT_REELS));
      assert.equal(new Set(reels.map(([, reel]) => reel.still)).size, reels.length);
      for (const [id, reel] of reels) {
        const figure = page.locator(`#reel-${id}.home-reel`);
        const video = figure.locator('video');
        assert.equal(await video.getAttribute('src'), '/artifacts/docs/' + reel.video);
        assert.equal(await video.getAttribute('poster'), '/artifacts/docs/' + reel.still);
        assert.ok(
          (await page.request.get(new URL('/artifacts/docs/' + reel.still, base).href)).ok(),
        );
        assert.ok(await video.evaluate((element: HTMLVideoElement) => element.loop));
        await video.evaluate(async (element: HTMLVideoElement) => {
          if (element.readyState < HTMLMediaElement.HAVE_METADATA)
            await new Promise<void>((resolve) =>
              element.addEventListener('loadedmetadata', () => resolve(), { once: true }),
            );
          element.pause();
        });
        const seek = (time: number) =>
          video.evaluate(async (element: HTMLVideoElement, seconds) => {
            if (element.currentTime === seconds) return;
            const sought = new Promise<void>((resolve) =>
              element.addEventListener('seeked', () => resolve(), { once: true }),
            );
            element.currentTime = seconds;
            await sought;
          }, time);
        for (const cue of reel.cues) {
          await seek((cue.start + cue.end) / 2);
          const caption = figure.locator('.home-reel-caption');
          await caption.waitFor();
          assert.equal(await caption.innerText(), cue.text);
        }
        if (engine === chromium && !mobile) {
          const cue = reel.cues.at(-1)!;
          await seek(Math.max(cue.start, cue.end - 1.2));
          await figure
            .locator('.home-reel-video')
            .screenshot({ path: resolve(output, `${id}@2x.png`) });
        }
        if (reel.cues[0].start > 0) {
          await seek(0);
          await figure.locator('.home-reel-caption').waitFor({ state: 'detached' });
        }
      }
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));

      const camera = page.locator('.sd-camera input');
      assert.equal(await camera.count(), release.firmwares.length);
      assert.ok(await camera.first().isChecked());
      const recipeGroup = page.getByRole('group', { name: 'Recipes' });
      const recipeToggle = recipeGroup.getByRole('checkbox', { name: /^Recipes/ });
      const recipeList = recipeGroup.locator('.home-recipe-list input');
      assert.match(
        await page.locator('.home-reels').innerText(),
        new RegExp(`\\b${recipes.length} RECIPES\\b`),
      );
      assert.equal((await recipeGroup.locator('small').innerText()).trim(), String(recipes.length));
      assert.ok(await recipeToggle.isChecked());
      assert.equal(await recipeToggle.evaluate((el: HTMLInputElement) => el.indeterminate), false);
      assert.equal(await recipeList.count(), recipes.length);
      assert.equal(
        await recipeGroup.locator('.home-recipe-list input:checked').count(),
        recipes.length,
      );

      if (engine === chromium && !mobile) {
        for (const [selector, name] of [
          ['.home-reels', 'reels-and-sd-card'],
          ['#software', 'emulator-top'],
        ]) {
          await page
            .locator(selector)
            .evaluate((element) => scrollBy(0, element.getBoundingClientRect().top - 20));
          await page.screenshot({ path: resolve(output, `${name}@2x.png`) });
        }
      }
      await recipeToggle.uncheck();
      assert.equal(await recipeGroup.locator('.home-recipe-list input:checked').count(), 0);
      await recipeToggle.check();
      assert.equal(
        await recipeGroup.locator('.home-recipe-list input:checked').count(),
        recipes.length,
      );
      await recipeList.first().uncheck();
      assert.ok(await recipeToggle.evaluate((el: HTMLInputElement) => el.indeterminate));

      for (const selector of [
        '.release-personal-note .manual-feature-link',
        '.home-reels .manual-feature-link',
        '.home-release-notes .release-notes .manual-feature-link',
      ]) {
        const link = page.locator(selector).first();
        if (!(await link.count())) continue;
        const href = await link.getAttribute('href');
        if (mobile) await link.tap();
        else await link.hover();
        const tooltip = page.locator('.catalog-card.on');
        await tooltip.waitFor();
        assert.equal(
          await tooltip.getByRole('link', { name: 'Read in the manual →' }).getAttribute('href'),
          href,
        );
        if (mobile) await page.touchscreen.tap(4, 4);
        else await page.keyboard.press('Escape');
        await tooltip.waitFor({ state: 'detached' });
      }

      if (engine === chromium && !mobile) {
        for (const input of await camera.all()) await input.check();
        await page.locator('.home-download-button').click();
        const button = page.getByRole('button', { name: /Download SD card files/ });
        const downloaded = page.waitForEvent('download');
        await button.click();
        const file = await downloaded;
        assert.equal(file.suggestedFilename(), `fujimod-sd-card-${release.release}.zip`);
        const files = unzipSync(new Uint8Array(await readFile(await file.path())));
        assert.equal(files[recipes[0].path], undefined, 'An unchecked recipe is excluded');
        for (const asset of release.sd_card_files ?? []) {
          if (asset.path !== recipes[0].path) assert.equal(digest(files[asset.path]), asset.sha256);
        }
        for (const camera of release.firmwares) {
          const firmware = sdBootV2Firmware(camera.id);
          const manifest = await loadOtaRelease(release.assetBase, firmware);
          const root = `FUJIMOD/SYSTEM/${firmware}`;
          const initial = JSON.parse(strFromU8(files[`${root}/A.JSN`]));
          assert.equal(initial.version, manifest.version);
          assert.ok(files[`${root}/STATE.JSN`]);
          for (const file of Object.values(manifest.fs))
            assert.equal(
              digest(files[`FUJIMOD/SYSTEM/STORE/${storeName(file.sha256)}`]),
              file.sha256,
            );
        }
      }

      await page.goto(new URL('/docs/manual/menus/', base).href, { waitUntil: 'networkidle' });
      const manualLink = page.locator('.manual-menu-tree .manual-feature-link').first();
      const href = await manualLink.getAttribute('href');
      if (mobile) await manualLink.tap();
      else await manualLink.focus();
      const manualCard = page.locator('.catalog-card.on');
      await manualCard.waitFor();
      await manualCard.getByRole('link', { name: 'Read in the manual →' }).click();
      await page.waitForURL(new URL(href!, base).href);
      assert.deepEqual(errors, [], `${engine.name()} page errors`);
      console.log(
        `${engine.name()} ${mobile ? 'phone' : 'desktop'}: layout, release, download selection, and shared cards passed`,
      );
      await page.close();
    }
  } finally {
    await browser.close();
  }
}
