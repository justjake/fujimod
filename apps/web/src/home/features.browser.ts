/** Check bounded preview playback and media release with real recordings. */
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium, webkit, type Locator, type Page } from 'playwright';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const dat = process.env.FUJIMOD_TEST_DAT;
const output = new URL('../../build/features/', import.meta.url);
await mkdir(output, { recursive: true });

async function playing(tile: Locator) {
  await tile.page().waitForFunction(
    (element) => {
      const video = element?.querySelector('video');
      return (
        video instanceof HTMLVideoElement &&
        !video.paused &&
        video.currentTime > 0 &&
        getComputedStyle(video).opacity === '1'
      );
    },
    await tile.elementHandle(),
  );
}
async function noVideo(page: Page) {
  await page.waitForFunction(() => !document.querySelector('.catalog-tile-video'));
}
async function boot(page: Page) {
  await page.locator('#firmware-file').setInputFiles(dat!);
  await page.waitForFunction(
    () =>
      document.querySelector('dialog.firmware-actions[open]') ||
      performance.getEntriesByName('camera-engine-boot').length > 0,
  );
  const launch = page.getByRole('button', { name: 'Launch camera', exact: true });
  if (await launch.isVisible()) await launch.click();
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
}

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch();
  try {
    for (const mobile of [false, true]) {
      const label = `${engine.name()}-${mobile ? 'mobile' : 'desktop'}`;
      const context = await browser.newContext({
        viewport: mobile ? { width: 390, height: 844 } : { width: 1200, height: 900 },
        hasTouch: mobile,
        isMobile: mobile,
      });
      // Observe releases without retaining removed elements or their decoded media.
      await context.addInitScript(() => {
        new MutationObserver(() => {
          if (document.querySelectorAll('.catalog-tile-video').length > 1)
            performance.mark('feature-video-budget-exceeded');
        }).observe(document, { childList: true, subtree: true });
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      console.log(`${label}: opening catalog`);
      await page.goto(base);
      await page.waitForFunction(
        () => performance.getEntriesByName('camera-catalog-ready').length > 0,
      );
      if (!mobile) await noVideo(page);
      const tiles = page
        .locator('.catalog-tile')
        .filter({ has: page.locator('.catalog-tile-preview') });
      assert.equal(await tiles.count(), 11);
      let previous: Awaited<ReturnType<Locator['elementHandle']>> | null = null;
      // Revisit every card to catch accumulated video elements and unreleased sources.
      for (let pass = 0; pass < (mobile ? 2 : 1); pass++) {
        for (let index = 0; index < (await tiles.count()); index++) {
          const tile = tiles.nth(index);
          const preview = tile.locator('.catalog-tile-preview');
          await preview.evaluate((element) => element.scrollIntoView({ block: 'center' }));
          await tile.locator('img').evaluate(async (element) => {
            await (element as HTMLImageElement).decode();
          });
          if (!mobile) {
            await noVideo(page);
            await preview.hover();
          }
          await playing(tile);
          if (previous) {
            assert.equal(
              await previous.evaluate((element) => {
                const video = element as HTMLVideoElement;
                return (
                  video.paused &&
                  !video.hasAttribute('src') &&
                  video.readyState === 0 &&
                  !video.isConnected
                );
              }),
              true,
            );
            await previous.dispose();
          }
          previous = mobile ? await tile.locator('video').elementHandle() : null;
          assert.equal(await page.locator('.catalog-tile-video').count(), 1);
          if (index === 0 && pass === 0)
            await page.screenshot({ path: new URL(`${label}-playing.png`, output).pathname });
          if (!mobile) {
            const previous = await tile.locator('video').elementHandle();
            await page.mouse.move(0, 0);
            await noVideo(page);
            assert.equal(
              await previous!.evaluate((element) => {
                const video = element as HTMLVideoElement;
                return (
                  video.paused &&
                  !video.hasAttribute('src') &&
                  video.readyState === 0 &&
                  !video.isConnected
                );
              }),
              true,
            );
            await previous!.dispose();
          }
        }
      }
      await page
        .locator('#workspace')
        .evaluate((element) => element.scrollIntoView({ block: 'start' }));
      await noVideo(page);
      if (previous) {
        assert.equal(
          await previous.evaluate((element) => {
            const video = element as HTMLVideoElement;
            return (
              video.paused &&
              !video.hasAttribute('src') &&
              video.readyState === 0 &&
              !video.isConnected
            );
          }),
          true,
        );
        await previous.dispose();
      }
      assert.equal(
        await page.evaluate(
          () => performance.getEntriesByName('feature-video-budget-exceeded').length,
        ),
        0,
      );
      if (mobile) {
        await page.setViewportSize({ width: 844, height: 390 });
        await tiles
          .first()
          .locator('.catalog-tile-preview')
          .evaluate((element) => element.scrollIntoView({ block: 'center' }));
        await page.waitForFunction(() => document.querySelector('.catalog-tile-video.playing'));
        assert.equal(await page.locator('.catalog-tile-video').count(), 1);
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
          true,
        );
        await page
          .locator('#workspace')
          .evaluate((element) => element.scrollIntoView({ block: 'start' }));
        await noVideo(page);
      } else {
        const recipe = tiles.filter({ hasText: 'Unlimited Recipes' });
        await recipe.focus();
        await playing(recipe);
        await recipe.evaluate((element) => (element as HTMLElement).blur());
        await noVideo(page);
      }
      if (mobile && dat) {
        await page.setViewportSize({ width: 390, height: 844 });
        const first = tiles.first();
        await first
          .locator('.catalog-tile-preview')
          .evaluate((element) => element.scrollIntoView({ block: 'center' }));
        await playing(first);
        const previous = await first.locator('video').elementHandle();
        console.log(`${label}: launching firmware and verifying catalog teardown`);
        await boot(page);
        assert.equal(await page.locator('#features').count(), 0);
        await noVideo(page);
        assert.equal(
          await previous!.evaluate((element) => {
            const video = element as HTMLVideoElement;
            return (
              video.paused &&
              !video.hasAttribute('src') &&
              video.readyState === 0 &&
              !video.isConnected
            );
          }),
          true,
        );
        await previous!.dispose();
      }
      assert.deepEqual(errors, []);
      console.log(
        `${label}: all previews play; one-video budget and source cleanup passed${mobile && dat ? '; firmware launch releases the catalog' : ''}`,
      );
      await context.close();
    }
  } finally {
    await browser.close();
  }
}
