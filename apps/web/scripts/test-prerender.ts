declare global {
  interface Window {
    __hydrationDOM: Element | null;
  }
}
import { createServer } from 'node:http';
import { readFile, readdir } from 'node:fs/promises';
import { resolve, sep, extname } from 'node:path';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
const root = resolve(process.argv[2] || 'dist/client');
for (const file of await readdir(root, { recursive: true })) {
  assert.ok(
    !/\.glb$|fujimod_simulation|(?:^|[\\/])environments[\\/]/.test(file),
    `Unexpected 3D asset: ${file}`,
  );
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://test');
  const path = resolve(
    root,
    '.' + decodeURIComponent(url.pathname),
    url.pathname.endsWith('/') ? 'index.html' : '',
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
        '.wasm': 'application/wasm',
        '.svg': 'image/svg+xml',
        '.png': 'image/png',
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
      for (const [route, text] of [
        ['/', 'FujiMod is not ready for your camera'],
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
          assert.equal(await page.locator('main.bug-report').count(), 1);
          assert.equal(await page.getByRole('img', { name: 'FujiMod', exact: true }).count(), 1);
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
          assert.ok((await page.locator('#camera-table .gen').count()) > 0);
        }
        await page.close();
      }
      await noJs.close();
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
      const note = page.locator('.personal-note');
      assert.match(await note.innerText(), /It is \d+ days old and changing too quickly/);
      assert.equal(await note.locator('mark, em, strong, b, svg').count(), 0);
      assert.equal(await note.locator('a').count(), 1);
      await note.getByRole('link', { name: 'camera emulator', exact: true }).click();
      assert.equal(new URL(page.url()).hash, '#workspace');
      await page.locator('#features .catalog-ref').first().focus();
      assert.equal(await page.locator('.catalog-card.on').count(), 1);
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
