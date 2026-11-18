declare global {
  interface Window {
    __hydrationDOM: Element | null;
  }
}
import { createServer } from 'node:http';
import { mkdir, readFile, readdir } from 'node:fs/promises';
import { resolve, sep, extname } from 'node:path';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { routes } from './prerender.ts';
import { manualChapters, manualMenuRows, manualPageAt } from '../src/docs/manual-data.ts';
import { manualMenus, manualPagePath, manualRoot } from '../src/docs/manual-routes.ts';
import { textPagePath } from '../src/text/routes.ts';
import credits from '../src/text/credits.generated.json' with { type: 'json' };
const root = resolve(process.argv[2] || 'dist/client');
const chapters = manualChapters();
const manualPage = chapters[0]?.pages[0];
assert.ok(manualPage, 'Expected a generated manual page');
const hoverProof = resolve('build/hover-cards');
await mkdir(hoverProof, { recursive: true });
for (const file of await readdir(root, { recursive: true })) {
  assert.ok(
    !/\.glb$|fujimod_simulation|(?:^|[\\/])environments[\\/]/.test(file),
    `Unexpected 3D asset: ${file}`,
  );
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://test');
  const textRoute = textPagePath(url.pathname);
  const path = resolve(
    root,
    textRoute ? `.${textRoute}.html` : '.' + decodeURIComponent(url.pathname),
    !textRoute && url.pathname.endsWith('/') ? 'index.html' : '',
  );
  if (!path.startsWith(root + sep)) {
    res.writeHead(403).end();
    return;
  }
  try {
    const bytes = await readFile(path);
    res.setHeader(
      'Content-Type',
      {
        '.html': 'text/html',
        '.js': 'text/javascript',
        '.css': 'text/css',
        '.json': 'application/json',
        '.txt': 'text/plain; charset=utf-8',
        '.wasm': 'application/wasm',
        '.svg': 'image/svg+xml',
        '.png': 'image/png',
        '.webp': 'image/webp',
      }[extname(path)] || 'application/octet-stream',
    );
    res.end(bytes);
  } catch {
    res.writeHead(404).end();
  }
});
await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
const base = `http://127.0.0.1:${(server.address() as import('node:net').AddressInfo).port}`;
try {
  for (const type of [chromium, webkit]) {
    const browser = await type.launch({ headless: true });
    try {
      const noJs = await browser.newContext({ javaScriptEnabled: false });
      for (const route of routes) {
        const page = await noJs.newPage();
        assert.equal((await page.goto(base + route))?.status(), 200);
        const header = page.locator('.site-header');
        assert.equal(await header.count(), 1);
        const manual = manualPageAt(route);
        if (manual) {
          const anchors = await page
            .locator('.manual-entry')
            .evaluateAll((entries) => entries.map((entry) => entry.id));
          assert.deepEqual(
            anchors,
            manual.sections.map((section) => section.anchor),
          );
        }
        assert.deepEqual(await header.locator('nav a').allTextContents(), [
          'Manual',
          'Emulator',
          'Safety',
          'Dev Tools',
          'Recovery',
        ]);
        assert.equal(
          await header.getByRole('link', { name: 'Discord' }).getAttribute('href'),
          'https://discord.gg/jWvFPakw7X',
        );
        const title = await page.title();
        const description = await page.locator('meta[name="description"]').getAttribute('content');
        assert.ok(description);
        assert.equal(
          await page.locator('meta[property="og:title"]').getAttribute('content'),
          title,
        );
        assert.equal(
          await page.locator('meta[name="twitter:title"]').getAttribute('content'),
          title,
        );
        for (const selector of [
          'meta[property="og:description"]',
          'meta[name="twitter:description"]',
        ])
          assert.equal(await page.locator(selector).getAttribute('content'), description);
        const canonical = 'https://fujimod.com' + route;
        assert.equal(await page.locator('link[rel="canonical"]').getAttribute('href'), canonical);
        assert.equal(
          await page.locator('meta[property="og:url"]').getAttribute('content'),
          canonical,
        );
        if (route.startsWith('/dev-tools/')) assert.ok(title.startsWith('Dev Tools: '));
        const hasPreview = (await page.locator('meta[property="og:image"]').count()) > 0;
        assert.equal(
          await page.locator('meta[name="twitter:card"]').getAttribute('content'),
          hasPreview ? 'summary_large_image' : 'summary',
        );
        if (route === '/' && hasPreview) {
          const image = await page.locator('meta[property="og:image"]').getAttribute('content');
          assert.ok(image);
          assert.equal(new URL(image).origin, 'https://fujimod.com');
          assert.equal((await noJs.request.get(base + new URL(image).pathname)).status(), 200);
          assert.equal(
            await page.locator('meta[name="twitter:image"]').getAttribute('content'),
            image,
          );
        } else {
          assert.equal(
            await page.locator('meta[property^="og:image"], meta[name^="twitter:image"]').count(),
            0,
          );
        }
        await page.close();
      }
      const safety = await noJs.newPage();
      await safety.goto(base + '/safety');
      const outline = safety.getByRole('navigation', { name: 'On this page' });
      assert.equal(await outline.evaluate((nav) => getComputedStyle(nav).position), 'sticky');
      assert.deepEqual(
        await outline.getByRole('link').allTextContents(),
        await safety.locator('main h2, main h3').allTextContents(),
      );
      const atomicUpdates = outline.getByRole('link', { name: 'Atomic updates' });
      await atomicUpdates.click();
      await safety.waitForURL(base + '/safety#atomic-updates');
      await safety.close();

      const creditsPage = await noJs.newPage();
      await creditsPage.goto(base + '/credits');
      assert.equal(await creditsPage.locator('main h1 + h2').count(), 1);
      const creditHeadings = [
        'GitHub Contributors',
        'Testing & Feedback',
        'Recipe Authors',
        'Open Source',
      ];
      assert.deepEqual(
        await creditsPage
          .getByRole('navigation', { name: 'On this page' })
          .getByRole('link')
          .allTextContents(),
        creditHeadings,
      );
      assert.equal(
        await creditsPage.locator('#github-contributors + ul li').count(),
        credits.contributors.length,
      );
      const contributors = creditsPage.locator('#github-contributors + ul');
      for (const name of ['heytchap', 'chris-rudmin']) {
        assert.equal(
          await contributors.getByRole('link', { name, exact: true }).getAttribute('href'),
          `https://github.com/${name}`,
        );
      }
      const testers = creditsPage.locator('#testing-feedback + ul');
      const jody = testers.getByRole('link', { name: 'jody' });
      assert.equal(await jody.getAttribute('href'), 'https://www.instagram.com/jody_hinterleitner');
      assert.equal(
        await jody.evaluate((node) => getComputedStyle(node).textDecorationLine),
        'underline',
      );
      assert.equal(await testers.locator('li').count(), 2);
      assert.equal(await testers.getByText('anonymous', { exact: true }).count(), 1);
      assert.equal(await testers.getByRole('link', { name: 'anonymous' }).count(), 0);
      assert.equal(
        await creditsPage.locator('.credits-recipes a').count(),
        credits.recipeAuthors.reduce((sum, author) => sum + author.recipes.length, 0),
      );
      const sbom = await (await noJs.request.get(base + '/sbom.json')).json();
      const licenseDetails = creditsPage.locator('.credits-license-details');
      assert.equal(await licenseDetails.getAttribute('open'), null);
      assert.equal(await creditsPage.locator('.credits-products li').count(), 3);
      await licenseDetails.locator('summary').click();
      assert.ok(await licenseDetails.locator('.credits-components').isVisible());
      assert.equal(
        await licenseDetails
          .locator('.credits-components-scroll')
          .evaluate((node) => getComputedStyle(node).maxHeight),
        '420px',
      );
      const components = creditsPage.locator('.credits-components tbody tr');
      assert.equal(await components.count(), sbom.components.length);
      const licenseHref = await components
        .first()
        .locator('a[href^="/licenses/"]')
        .getAttribute('href');
      assert.ok(licenseHref);
      const license = await noJs.request.get(base + licenseHref);
      assert.equal(license.status(), 200);
      assert.ok((await license.text()).trim().length > 100);
      await creditsPage.close();

      const releases = await noJs.newPage();
      await releases.goto(base + '/releases');
      const releaseDates = await releases
        .locator('main time')
        .evaluateAll((elements) => elements.map((time) => (time as HTMLTimeElement).dateTime));
      assert.ok(releaseDates.length, 'Expected release history');
      assert.deepEqual(
        releaseDates,
        [...releaseDates].sort((a, b) => b.localeCompare(a)),
      );
      assert.equal(
        await releases.getByRole('navigation', { name: 'On this page' }).getByRole('link').count(),
        releaseDates.length,
      );
      await releases.close();

      const docs = await noJs.newPage();
      await docs.goto(base + manualRoot);
      const sidebar = docs.getByRole('navigation', { name: 'Manual' });
      const outlinePages = docs.locator('.manual-chapters li');
      assert.deepEqual(
        await outlinePages.evaluateAll((items) =>
          items.map((item) => [
            item.querySelector('a')?.getAttribute('href'),
            item.querySelector('span')?.textContent,
          ]),
        ),
        chapters.flatMap((chapter) => chapter.pages.map((link) => [link.href, link.summary])),
      );
      assert.equal(await docs.locator('main video').count(), 0);
      assert.equal(await sidebar.locator('.docs-home').count(), 0);
      await sidebar.locator(`a[href="${manualPage.href}"]`).click();
      await docs.waitForURL(base + manualPage.href);
      assert.equal(await sidebar.locator('[aria-current="page"]').innerText(), manualPage.title);
      assert.equal(await sidebar.locator('.docs-home').count(), 0);
      assert.equal(await docs.locator('.manual-introduction ol').count(), 1);
      await sidebar.getByRole('link', { name: 'Menu index' }).click();
      await docs.waitForURL(base + manualMenus);
      assert.equal(await docs.locator('main h1').innerText(), 'Menu index');
      const menuLinks = docs.locator('.manual-menu-tree a');
      assert.deepEqual(
        await menuLinks.evaluateAll((links) =>
          links.map((link) => [link.textContent, link.getAttribute('href')]),
        ),
        manualMenuRows.map((row) => [row.title, `${manualPagePath(row.page)}#${row.anchor}`]),
      );
      const alias = manualMenuRows.find((row) => {
        const section = manualPageAt(manualPagePath(row.page))?.sections.find(
          (entry) => entry.anchor === row.anchor,
        );
        return section?.paths
          ?.slice(1)
          .some((path) => path.join(' › ') === [...row.path, row.title].join(' › '));
      });
      assert.ok(alias, 'Expected a menu row with an alternate path');
      const destination = `${manualPagePath(alias.page)}#${alias.anchor}`;
      await menuLinks
        .and(docs.locator(`a[href="${destination}"]`))
        .last()
        .click();
      await docs.waitForURL(base + destination);
      const paths = await docs.locator(`#${alias.anchor} .manual-paths li`).allTextContents();
      assert.ok(paths.length > 1);
      assert.ok(paths.includes([...alias.path, alias.title].join(' › ')));
      await docs.goto(base + '/docs/sdk/focal/');
      assert.equal(await docs.getByRole('navigation', { name: 'FocalUI' }).count(), 1);
      await docs.close();
      assert.equal((await noJs.request.get(base + '/setup/')).status(), 404);
      for (const [route, text] of [
        ['/', 'Today we’re releasing the second version of Fujimod'],
        ['/dev-tools/', 'Dev Tools'],
        ['/bug/', undefined],
      ]) {
        const page = await noJs.newPage();
        const response = await page.goto(base + route);
        assert.equal(response!.status(), 200);
        if (text)
          assert.ok(
            (await page.locator('body').innerText()).includes(text),
            `${type.name()} ${route}: static content`,
          );
        else {
          assert.equal(await page.locator('.shell.bug-report').count(), 1);
          assert.equal(await page.getByRole('img', { name: 'Fujimod', exact: true }).count(), 1);
        }
        if (route === '/') {
          assert.equal(await page.locator('#workspace').count(), 1);
          assert.equal(await page.locator('#simulation-canvas,#view-3d,#view-lcd').count(), 0);
          assert.equal(await page.locator('#lcd-source').isVisible(), false);
          assert.equal(await page.locator('#idle-cameras').count(), 0);
          assert.equal(
            await page.getByRole('button', { name: 'New camera', exact: true }).count(),
            0,
          );
          assert.equal(await page.locator('#camera-table').count(), 1);
          assert.equal(await page.locator('header .site-nav').count(), 1);
          assert.equal(await page.locator('.release-personal-note').count(), 1);
          assert.equal(await page.locator('.home-reel').count(), 3);
          assert.equal(await page.locator('.home-download').count(), 1);
          assert.equal(
            await page
              .locator('.release-showcase,.release-highlight,.release-bodies,#quickstart')
              .count(),
            0,
          );
        }
        await page.close();
      }
      await noJs.close();
      const touch = await browser.newContext({
        viewport: { width: 390, height: 844 },
        hasTouch: true,
        isMobile: true,
      });
      const touchPage = await touch.newPage();
      await touchPage.goto(base + '/');
      await touchPage.waitForFunction(
        () => performance.getEntriesByName('camera-catalog-ready').length > 0,
      );
      const trigger = touchPage.locator('.home-reels .manual-feature-link').first();
      await trigger.tap();
      await touchPage.locator('.catalog-card.on').waitFor();
      await touchPage.screenshot({ path: resolve(hoverProof, `${type.name()}-touch.png`) });
      await touchPage.touchscreen.tap(4, 4);
      await touchPage.locator('.catalog-card.on').waitFor({ state: 'detached' });
      await touch.close();
      const context = await browser.newContext();
      const page = await context.newPage();
      const runtimeRequests: string[] = [],
        workers: string[] = [];
      page.on('worker', (worker) => workers.push(worker.url()));
      page.on('request', (request) => {
        if (/\/runtime\/(?!catalog\.json)|default-scene|wasm-worker/.test(request.url()))
          runtimeRequests.push(request.url());
      });
      const errors: string[] = [],
        diagnostics: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (msg) => {
        if (/hydration|unclaimed|\[[A-Z_]+\]/i.test(msg.text())) diagnostics.push(msg.text());
      });
      // Browser extensions add nodes to <head> while it is parsed; hydration must not walk it.
      await page.addInitScript(() => {
        new MutationObserver((_, observer) => {
          if (!document.head) return;
          document.head.prepend(document.createElement('style'));
          observer.disconnect();
        }).observe(document, { childList: true, subtree: true });
      });
      // The static-host test exercises hydration/UI; firmware execution has its own real-runtime gate.
      await page.addInitScript(() => {
        window.__hydrationDOM = null;
        document.addEventListener('DOMContentLoaded', () => {
          window.__hydrationDOM = document.querySelector('#workspace');
        });
      });
      await page.goto(base + '/');
      try {
        await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
      } catch (error) {
        console.log(type.name(), { errors, diagnostics });
        console.log(
          await page.evaluate(() => ({
            body: document.body.innerText,
            scripts: [...document.scripts].map((script) => ({
              src: script.src,
              type: script.type,
            })),
            feedback: document.querySelector('#feedback-open')?.outerHTML,
          })),
        );
        throw error;
      }
      assert.deepEqual(runtimeRequests, [], 'The idle homepage must not download the emulator');
      assert.deepEqual(workers, [], 'The idle homepage must not create an emulator worker');
      assert.equal(await page.locator('#workspace').count(), 1);
      assert.ok(
        await page.evaluate(() => window.__hydrationDOM === document.querySelector('#workspace')),
        'hydrate existing homepage nodes',
      );
      assert.equal(await page.locator('header .site-nav').count(), 1);
      assert.equal(await page.locator('.release-personal-note').count(), 1);
      assert.equal(await page.locator('h1').count(), 0);
      assert.equal(await page.locator('.home-download').count(), 1);
      assert.equal(
        await page
          .locator('.release-showcase,.release-highlight,.release-bodies,#quickstart')
          .count(),
        0,
      );
      await page.locator('.home-reels .manual-feature-link').first().focus();
      await page.locator('.catalog-card.on').waitFor();
      await page.keyboard.press('Escape');
      await page.locator('.catalog-card.on').waitFor({ state: 'detached' });
      const desktopTrigger = page.locator('.home-reels .manual-feature-link').first();
      await desktopTrigger.hover();
      await page.locator('.catalog-card.on').waitFor();
      await page.mouse.move(0, 0);
      await page.waitForTimeout(300);
      assert.equal(await page.locator('.catalog-card.on').count(), 0);
      // The idle homepage has no camera bar; feedback opens from its URL.
      await page.goto(base + '/?feedback');
      await page.locator('#feedback-dialog[open]').waitFor();
      assert.equal(
        await page.locator('#feedback-dialog').evaluate((node) => (node as HTMLDialogElement).open),
        true,
      );
      await page.locator('#feedback-description').fill('Static hydration check');
      await page.locator('#feedback-dialog button[value="cancel"]').click();
      assert.equal(
        await page.locator('#feedback-dialog').evaluate((node) => (node as HTMLDialogElement).open),
        false,
      );
      await page.goto(base + '/dev-tools/');
      const connect = page.getByRole('button', { name: 'Connect camera', exact: true });
      await connect.waitFor();
      assert.equal(await connect.isEnabled(), await page.evaluate(() => 'usb' in navigator));
      await page.getByRole('button', { name: 'Show instructions', exact: true }).click();
      await page.waitForURL(base + '/dev-tools/service/');
      await page.goto(base + '/bug/');
      await page
        .getByRole('alert')
        .filter({ hasText: 'This link has no valid camera report.' })
        .waitFor();
      assert.equal(await page.locator('meta[name="fujimod-local-reports"]').count(), 0);
      await page.goto(base + manualRoot);
      const search = page.getByRole('searchbox', { name: 'Search the manual' });
      const chapterLink = page.locator(`.docs-sidebar a[href="${manualPage.href}"]`);
      await search.fill('zzzzzzzz');
      await chapterLink.waitFor({ state: 'detached' });
      await search.clear();
      await chapterLink.click();
      await page.waitForURL(base + manualPage.href);
      assert.deepEqual(errors, [], `${type.name()} page errors`);
      assert.deepEqual(diagnostics, [], `${type.name()} hydration/reactivity diagnostics`);
      await context.close();
      console.log(
        `${type.name()}: three static pages, existing-node hydration, features and feedback passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  await new Promise<void>((done) => server.close(() => done()));
}
