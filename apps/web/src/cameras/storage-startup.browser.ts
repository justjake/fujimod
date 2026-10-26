/** Exercise native IndexedDB aborts and the idle viewer in Chromium and WebKit. */
import { chromium, webkit, devices } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to a development homepage.');
const output = new URL('../../build/storage-startup/', import.meta.url);
await mkdir(output, { recursive: true });

for (const type of [chromium, webkit]) {
  const browser = await type.launch();
  try {
    const context = await browser.newContext({ ...devices['iPhone 13'] });
    await context.routeWebSocket('**/*', () => {});
    // Exercise the published page's storage behavior without development reporting.
    await context.route('**/*', async (route) => {
      if (route.request().resourceType() !== 'document') return route.continue();
      const response = await route.fetch();
      const body = (await response.text())
        // Server rendering adds hydration attributes (`_hk`) to these tags.
        .replace(/<meta [^>]*name="fujimod-local-reports"[^>]*>/g, '')
        .replace(
          /<script [^>]*src="\/__(?:simulator-reports|browser-diagnostics)\.js"[^>]*><\/script>/g,
          '',
        );
      await route.fulfill({ response, body });
    });
    await context.addInitScript(() => {
      const getAll = IDBObjectStore.prototype.getAll;
      IDBObjectStore.prototype.getAll = function (...args) {
        const request = getAll.apply(this, args);
        if (this.transaction.db.name === 'fujimod-cameras') {
          const root = document.documentElement;
          const count = Number(root.dataset.cameraReads ?? 0) + 1;
          root.dataset.cameraReads = String(count);
          if (count === 1 || location.search.includes('abort-reads')) this.transaction.abort();
        }
        return request;
      };
    });
    const page = await context.newPage();
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(base);
    await page.waitForFunction(() => performance.getEntriesByName('camera-catalog-ready').length);
    assert.equal(await page.locator('html').getAttribute('data-camera-reads'), '2');
    assert.equal(await page.locator('.camera-storage-status.error').count(), 0);
    assert.equal(await page.locator('#lcd-source').isVisible(), false);
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    const bounds = await page.locator('#viewer').boundingBox();
    const launcher = await page.locator('.launcher').boundingBox();
    assert.ok(bounds && launcher && bounds.height <= launcher.height + 80);
    assert.equal(
      await page.locator('#workspace').evaluate((el) => getComputedStyle(el).borderTopWidth),
      '0px',
      'The idle launcher fills a phone without a stage frame',
    );
    assert.equal(
      await page.locator('#lcd-source').evaluate((el) => getComputedStyle(el).touchAction),
      'auto',
    );
    const before = await page.evaluate(() => scrollY);
    if (type === chromium) {
      const session = await context.newCDPSession(page);
      // Swipe on the steps: the supported-camera list scrolls on its own.
      const steps = (await page.locator('.launcher-actions').boundingBox())!;
      const x = steps.x + 8;
      const y = steps.y + steps.height / 2;
      await session.send('Input.dispatchTouchEvent', {
        type: 'touchStart',
        touchPoints: [{ x, y }],
      });
      for (let distance = 20; distance <= 160; distance += 20)
        await session.send('Input.dispatchTouchEvent', {
          type: 'touchMove',
          touchPoints: [{ x, y: y - distance }],
        });
      await session.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
      await session.detach();
    } else {
      await page.keyboard.press('PageDown');
    }
    await page.waitForFunction((previous) => scrollY > previous + 50, before);
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    const chooser = page.waitForEvent('filechooser');
    await page.locator('#firmware-prompt').click();
    await (await chooser).setFiles([]);
    await page.screenshot({ path: new URL(`${type.name()}-idle.png`, output).pathname });

    const result = await page.evaluate(async () => {
      const { CameraStore }: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      const store = new CameraStore();
      const file = new File([new Uint8Array([1, 2, 3])], 'storage-test.dat');
      const firmware = { id: 'storage-test', model: 'Storage test', version: '1', sha256: 'test' };
      const camera = await store.create(file, firmware, false);
      const another = { ...firmware, sha256: 'test-2' };
      const add = IDBObjectStore.prototype.add;
      let writes = 0;
      IDBObjectStore.prototype.add = function (...args) {
        if (this.name === 'library-bytes') {
          writes++;
          throw new DOMException('Injected write abort', 'AbortError');
        }
        return add.apply(this, args);
      };
      let failed = false;
      try {
        await store.create(file, another, false);
      } catch (error) {
        failed = error instanceof DOMException && error.name === 'AbortError';
      } finally {
        IDBObjectStore.prototype.add = add;
      }
      return { writes, failed, camera, records: await store.list() };
    });
    assert.equal(result.writes, 1, 'Aborted writes must never be replayed');
    assert.equal(result.failed, true);
    assert.deepEqual(
      result.records,
      [result.camera],
      'An aborted write preserves existing cameras',
    );

    const failing = new URL(base);
    failing.searchParams.set('abort-reads', '1');
    await page.goto(failing.href);
    await page.locator('.camera-storage-status.error').waitFor();
    assert.equal(await page.locator('html').getAttribute('data-camera-reads'), '2');
    await page.goto(base);
    await page.locator('.resume-camera').waitFor();
    assert.equal(await page.locator('.resume-camera').count(), 1);
    assert.equal(await page.locator('.camera-storage-status.error').count(), 0);
    assert.deepEqual(errors, []);

    await page.setViewportSize({ width: 1440, height: 1080 });
    await page.locator('#workspace').scrollIntoViewIfNeeded();
    assert.equal(await page.locator('#lcd-source').isVisible(), false);
    const desktop = await page.locator('#viewer').boundingBox();
    const desktopLauncher = await page.locator('.launcher').boundingBox();
    assert.ok(desktop && desktopLauncher && desktop.height <= desktopLauncher.height + 80);
    console.log(
      `${type.name()}: transient read recovered; persistent error shown; write atomic and unrepeated; idle viewer fits the launcher, scrolls and opens chooser`,
    );
    await context.close();

    // A version 1 database, with a firmware copy per camera, becomes one library entry.
    const upgradeContext = await browser.newContext();
    const upgradePage = await upgradeContext.newPage();
    // A static asset keeps the app from opening the database first.
    await upgradePage.goto(new URL('brand.png', base).href);
    const upgraded = await upgradePage.evaluate(async () => {
      await new Promise<void>((resolve, reject) => {
        const open = indexedDB.open('fujimod-cameras', 1);
        open.onupgradeneeded = () => {
          open.result.createObjectStore('cameras', { keyPath: 'id' });
          open.result.createObjectStore('firmware');
          open.result.createObjectStore('saves');
        };
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const db = open.result;
          const tx = db.transaction(['cameras', 'firmware'], 'readwrite');
          for (const [id, createdAt] of [
            ['first', 1],
            ['second', 2],
          ] as const) {
            tx.objectStore('cameras').add({
              id,
              name: 'Upgrade test · 1',
              firmwareId: 'upgrade-test',
              firmwareHash: 'upgrade',
              fileName: 'upgrade.dat',
              createdAt,
              revision: 0,
              modEnabled: false,
            });
            tx.objectStore('firmware').add(new Uint8Array([4, 5, 6]).buffer, id);
          }
          tx.oncomplete = () => {
            db.close();
            resolve();
          };
          tx.onerror = () => reject(tx.error);
        };
      });
      const { CameraStore }: typeof import('./store') = await import('/src/' + 'cameras/store.ts');
      const store = new CameraStore();
      const library = await store.library();
      const bytes = [...new Uint8Array(await (await store.firmware('upgrade')).arrayBuffer())];
      const cameras = (await store.list()).map((camera) => camera.id);
      const stores = await new Promise<string[]>((resolve, reject) => {
        const open = indexedDB.open('fujimod-cameras');
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          resolve([...open.result.objectStoreNames]);
          open.result.close();
        };
      });
      return { library, bytes, cameras, stores };
    });
    assert.deepEqual(
      upgraded.library.map((entry) => [entry.sha256, entry.firmwareId, entry.name]),
      [['upgrade', 'upgrade-test', 'Upgrade test · 1']],
    );
    assert.deepEqual(upgraded.bytes, [4, 5, 6]);
    assert.deepEqual(upgraded.cameras.sort(), ['first', 'second']);
    assert.deepEqual(upgraded.stores.sort(), ['cameras', 'library', 'library-bytes', 'saves']);
    console.log(`${type.name()}: version 1 cameras share one library firmware after upgrade`);
    await upgradeContext.close();
  } finally {
    await browser.close();
  }
}
