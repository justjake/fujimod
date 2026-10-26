import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium, webkit, devices } from 'playwright';

const base = process.env.FUJIMOD_WEB_URL || 'http://127.0.0.1:5201/';
await mkdir('build/development-hydration', { recursive: true });
for (const engine of [chromium, webkit]) {
  const browser = await engine.launch();
  try {
    const context = await browser.newContext({
      ...devices['iPhone 13'],
    });
    const page = await context.newPage();
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (/Hydration (?:structure|key|tag)|REACTIVITY_HALTED/.test(message.text()))
        errors.push(message.text());
    });
    let staleImports = 0;
    // Model Safari retaining the old unversioned preload. The HTML's preload
    // and hydration map must both bypass it; changing only the hint is unsafe.
    await page.route(/\/src\/(home|recovery)\/Page\.tsx$/, (route) => {
      staleImports++;
      return route.fulfill({
        contentType: 'text/javascript',
        body: "throw Error('Stale unversioned homepage imported'); export default function Home() {}",
      });
    });
    for (let visit = 0; visit < 2; visit++) {
      const response = await page.goto(base);
      assert.equal(response!.status(), 200);
      assert.equal(response!.headers()['cache-control'], 'no-store');
      const html = await response!.text();
      const preload = /<link rel="modulepreload" href="([^"]*\/src\/home\/Page\.tsx[^"]*)"/.exec(
        html,
      )?.[1];
      assert.ok(preload, 'homepage has a module preload');
      assert.match(preload, /development-revision=[a-f0-9]{64}/);
      assert.ok(
        html.includes(`:"${preload}"`),
        'hydration imports the same version as the preload',
      );
      await page.locator('.launcher').waitFor();
      const search = page.getByRole('searchbox', { name: 'Search cameras' });
      await search.fill('X-E2S');
      await page.getByRole('link', { name: 'Download X-E2S firmware from Fujifilm' }).waitFor();
      assert.equal(
        await page.getByRole('link', { name: 'Download X70 firmware from Fujifilm' }).count(),
        0,
      );
      if (visit === 0) {
        await search.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `build/development-hydration/${engine.name()}.png` });
      }
      await page.locator('#features .catalog-ref').first().focus();
      await page.locator('.catalog-card.on').waitFor();
      assert.equal(staleImports, 0);
      assert.deepEqual(errors, [], `${engine.name()} visit ${visit}: page initializes`);
    }
    await page.goto(new URL('recovery/', base).href);
    const system = page.getByRole('group', { name: 'System', exact: true });
    await system.getByRole('button', { name: 'Windows', exact: true }).click();
    await system.locator('button[aria-pressed="true"]').filter({ hasText: 'Windows' }).waitFor();
    assert.equal(
      await system
        .getByRole('button', { name: 'Windows', exact: true })
        .getAttribute('aria-pressed'),
      'true',
    );
    await system.getByRole('button', { name: 'Linux', exact: true }).click();
    await system.locator('button[aria-pressed="true"]').filter({ hasText: 'Linux' }).waitFor();
    assert.equal(
      await system.getByRole('button', { name: 'Linux', exact: true }).getAttribute('aria-pressed'),
      'true',
    );
    assert.equal(staleImports, 0);
    assert.deepEqual(errors, [], `${engine.name()} recovery initializes`);
    await context.close();
    console.log(`${engine.name()}: mobile hydration and inputs bypass stale unversioned page code`);
  } finally {
    await browser.close();
  }
}
