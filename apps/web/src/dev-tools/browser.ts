import { browserScript } from '../../scripts/browser-script.ts';
declare global {
  interface Window {
    consoleFixture: ReturnType<typeof import('./usb-fixture.ts').installConsoleFixture>;
  }
}
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
let base = process.env.FUJIMOD_WEB_URL;
let server;
try {
  if (!base) {
    base = 'http://127.0.0.1:5199/';
    const web = new URL('../../', import.meta.url);
    server = spawn(
      process.execPath,
      [
        fileURLToPath(new URL('node_modules/vite-plus/bin/vp', web)),
        'preview',
        '--host',
        '127.0.0.1',
        '--port',
        '5199',
        '--strictPort',
      ],
      { cwd: fileURLToPath(web), stdio: 'pipe', detached: process.platform !== 'win32' },
    );
    let output = '';
    server.stdout.on('data', (value) => {
      output += value;
    });
    server.stderr.on('data', (value) => {
      output += value;
    });
    const deadline = Date.now() + 15000;
    let ready = false;
    while (Date.now() < deadline && server.exitCode === null) {
      try {
        ready = (await fetch(base)).ok;
      } catch {
        /* Server is starting. */
      }
      if (ready) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (!ready) throw Error(`Preview did not start: ${output}`);
  }
  const url = new URL('dev-tools/', base).href;
  const directory = new URL('../../build/dev-tools-browser/', import.meta.url);
  await mkdir(directory, { recursive: true });
  const fixture = await browserScript(new URL('./usb-fixture.ts', import.meta.url));
  const upstream = await browserScript(
    new URL('../../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
  );
  for (const mobile of [false, true]) {
    const browser = await chromium.launch({ headless: true });
    try {
      const context = await browser.newContext({
        viewport: mobile ? { width: 390, height: 844 } : { width: 1300, height: 900 },
        isMobile: mobile,
        hasTouch: mobile,
      });
      await context.addInitScript(() => {
        Object.defineProperty(navigator, 'usb', {
          configurable: true,
          value: Object.assign(new EventTarget(), {
            getDevices: async () => [],
            requestDevice() {
              throw Error('USB fixture has not been installed');
            },
          }),
        });
      });
      await context.grantPermissions(
        ['local-network-access', 'clipboard-read', 'clipboard-write'],
        { origin: new URL(url).origin },
      );
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.route('**/__test__/upstream.mjs', (route) =>
        route.fulfill({ contentType: 'text/javascript', body: upstream }),
      );
      await page.route('**/__test__/console.mjs', (route) =>
        route.fulfill({
          contentType: 'text/javascript',
          body: fixture.replace(
            '../../../../bindings/javascript/tests/webusb-fixture.ts',
            './upstream.mjs',
          ),
        }),
      );
      await page.goto(url);
      await page.getByRole('heading', { name: 'Enter service mode', exact: true }).waitFor();
      await page.screenshot({
        path: new URL(mobile ? 'start-mobile.png' : 'start-desktop.png', directory).pathname,
        fullPage: true,
      });
      await page.evaluate(async () => {
        window.consoleFixture = (
          (await import('/__test__/' + 'console.mjs')) as typeof import('./usb-fixture.ts')
        ).installConsoleFixture({
          1: 'FUJIFILM',
          2: 'FUJIFILM',
          4: 'X-T1',
          8: '2.30',
          16: 'CAMERA',
          64: '1.00',
        });
        // Debug mode off: on the ff80 link that can only mean service mode, like the
        // owner's X100V.
        window.consoleFixture.state.configuration.set('0:247', 0);
      });
      await page
        .locator('.td-connects')
        .getByRole('button', { name: 'Connect camera', exact: true })
        .click();
      // The identity read runs automatically when the camera appears.
      await page.locator('.wb-camera dl').waitFor();
      assert.equal(
        await page.evaluate(
          () => window.consoleFixture.log.filter((entry) => entry[0] === 'picker').length,
        ),
        1,
      );
      await page.locator('.wb-state', { hasText: 'Idle' }).waitFor();
      const update = page.locator('.wb-firmware-update');
      assert.match(await update.innerText(), /Unknown firmware version appears to be outdated/);
      assert.equal(
        await update.getByRole('link').getAttribute('href'),
        'https://www.fujifilm-x.com/global/support/download/firmware/cameras/x-t1/',
      );
      await page.locator('.wb-service', { hasText: 'restores factory settings' }).waitFor();
      assert.equal(await page.locator('.wb-debug-state').innerText(), 'Debug mode: Off');
      assert.equal(await page.getByRole('button', { name: 'Disable debug mode' }).count(), 0);
      assert.equal(await page.getByRole('button', { name: 'Enable debug mode' }).count(), 1);
      const log = page.getByRole('region', { name: 'Connection log' });
      assert.ok((await log.innerText()).includes('Device info'), 'identity exchange is logged');
      // One camera: no camera column and no camera filters.
      assert.equal(await page.locator('.cl-camera').count(), 0);
      assert.equal(await page.getByRole('group', { name: 'Log cameras' }).count(), 0);

      console.log(`${mobile ? 'mobile' : 'desktop'}: camera ready, dumping configuration`);
      await page.getByRole('button', { name: 'Dump configuration', exact: true }).click();
      await page.getByRole('button', { name: 'Dump configuration', exact: true }).waitFor();
      await page.locator('.wb-state', { hasText: 'Idle' }).waitFor();

      await page.getByLabel('Console command', { exact: true }).fill('help');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await log.getByText('Fuji console ready', { exact: false }).first().waitFor();

      // Download all is a zip: a manifest plus a folder per camera of binary files.
      await page.locator('.files .tree', { hasText: 'configuration.bin' }).waitFor();
      const downloaded = page.waitForEvent('download');
      await page.getByRole('button', { name: 'Download all', exact: true }).click();
      const bundle = await readFile((await (await downloaded).path())!);
      assert.equal(bundle.readUInt32LE(0), 0x04034b50, 'Download all is a zip');
      const text = bundle.toString('latin1');
      for (const name of ['manifest.json', '/configuration.bin', '/log.jsonl'])
        assert.ok(text.includes(name), `archive contains ${name}`);

      // The Connection layer filters like every other layer.
      const connection = page.getByRole('group', { name: 'Log layers' }).getByRole('button', {
        name: 'Connection',
        exact: true,
      });
      await connection.click();
      assert.equal(await connection.getAttribute('aria-pressed'), 'false');
      await connection.click();

      const geometry = await page.evaluate(() => ({
        overflow: document.documentElement.scrollWidth > innerWidth,
        tools: document.querySelector('.wb-tools')!.getBoundingClientRect().width,
      }));
      assert.equal(geometry.overflow, false);
      if (!mobile) assert.equal(Math.round(geometry.tools), 420);
      await page.screenshot({
        path: new URL(mobile ? 'workbench-mobile.png' : 'workbench-desktop.png', directory)
          .pathname,
        fullPage: true,
      });
      assert.deepEqual(errors, []);
      console.log(
        `PASS ${mobile ? 'mobile' : 'desktop'}: connect, automatic identity, service-mode banner, configuration dump, console command, binary bundle, log filters`,
      );
    } finally {
      await browser.close();
    }
  }
  const browser = await webkit.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    await page.goto(url);
    const tile = page.getByRole('button', { name: 'Connect camera', exact: true });
    await tile.waitFor();
    assert.equal(await tile.isDisabled(), true);
    assert.ok(
      (await page.locator('.td-unsupported').textContent())?.includes('Use Chrome or Edge'),
    );
    console.log('PASS WebKit: without WebUSB the page says why and the camera tile is unavailable');
  } finally {
    await browser.close();
  }
} finally {
  if (server) {
    if (process.platform === 'win32') server.kill();
    else if (server.pid !== undefined) {
      try {
        process.kill(-server.pid, 'SIGTERM');
      } catch (error) {
        if (!(error instanceof Error && 'code' in error && error.code === 'ESRCH')) {
          console.error('Could not stop browser test preview:', error);
          process.exitCode = 1;
        }
      }
    }
    server.stdout.destroy();
    server.stderr.destroy();
  }
}
