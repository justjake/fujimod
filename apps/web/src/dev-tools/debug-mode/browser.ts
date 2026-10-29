import { chromium, webkit, type APIResponse } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { validateCatalog, type Catalog } from '../../catalog.ts';
import { browserScript } from '../../../scripts/browser-script.ts';

declare global {
  interface Window {
    serviceGuideFixture: ReturnType<typeof import('../usb-fixture.ts').installConsoleFixture>;
    serviceGuidePicker: { accept(): void; cancel(): void };
  }
}

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the development web app URL.');
const output = new URL('../../../build/service-mode-guide/', import.meta.url);
await mkdir(output, { recursive: true });
const fixture = await browserScript(new URL('../usb-fixture.ts', import.meta.url));
const upstream = await browserScript(
  new URL('../../../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
);
for (const browserType of [chromium, webkit]) {
  const browser = await browserType.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1300, height: 1100 } });
    if (browserType === chromium) {
      await page.addInitScript(() => {
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
    }
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (message.type() === 'warning' && /hydration/i.test(message.text()))
        errors.push(message.text());
    });
    await page.goto(new URL('dev-tools/service/', base).href);
    await page.getByRole('button', { name: 'Begin', exact: true }).click();
    const connect = page.getByRole('button', {
      name: 'Try to Connect',
      exact: true,
    });
    const guide = page.locator('.dm');
    assert.doesNotMatch(await guide.innerText(), /green|lamp|LED/);
    assert.equal(await guide.locator('input[type="checkbox"]').count(), 0);
    await page.screenshot({
      path: new URL(`${browserType.name()}-instructions.png`, output).pathname,
      animations: 'disabled',
    });
    if (browserType === chromium) {
      assert.equal(await connect.isEnabled(), true);
      await page.evaluate(async () => {
        window.serviceGuideFixture = (
          (await import('/__test__/' + 'console.mjs')) as typeof import('../usb-fixture.ts')
        ).installConsoleFixture({
          1: 'FUJIFILM',
          2: 'FUJIFILM',
          4: 'X70',
          8: '1.39',
          16: 'CAMERA',
          64: '1.00',
        });
        const usb = (
          navigator as Navigator & {
            usb: {
              getDevices(): Promise<unknown[]>;
              requestDevice(options: unknown): Promise<unknown>;
            };
          }
        ).usb;
        let permitted = false;
        const devices = usb.getDevices.bind(usb);
        usb.getDevices = () => (permitted ? devices() : Promise.resolve([]));
        const request = usb.requestDevice.bind(usb);
        usb.requestDevice = (options) => {
          const selected = request(options);
          return new Promise((resolve, reject) => {
            window.serviceGuidePicker = {
              accept: () => {
                permitted = true;
                void selected.then(resolve, reject);
              },
              cancel: () => reject(new DOMException('No device selected', 'NotFoundError')),
            };
          });
        };
      });
      await connect.click();
      assert.equal(await connect.isDisabled(), true);
      assert.equal(
        await page.getByRole('heading', { name: 'Restart into service mode' }).count(),
        1,
      );
      assert.equal(await page.getByRole('button', { name: 'Open workbench' }).count(), 0);
      const help = page.getByText('Can’t see your camera?', { exact: false });
      assert.equal(await help.count(), 0);
      await help.waitFor();
      await help.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: new URL('chromium-picker-help.png', output).pathname,
        animations: 'disabled',
      });
      await page.evaluate(() => window.serviceGuidePicker.cancel());
      await connect.waitFor();
      await page.waitForFunction(() =>
        [...document.querySelectorAll<HTMLButtonElement>('.dm button')].some(
          (button) => button.textContent === 'Try to Connect' && !button.disabled,
        ),
      );
      assert.equal(await page.getByRole('button', { name: 'Open workbench' }).count(), 0);
      await connect.click();
      assert.equal(await help.count(), 0);
      await page.evaluate(() => window.serviceGuidePicker.accept());
      await page.getByRole('button', { name: 'Open workbench', exact: true }).click();
      await page.locator('.wb-camera h2').waitFor();
      assert.equal(await page.locator('.wb-camera h2').innerText(), 'X70');
      assert.equal(new URL(page.url()).pathname, '/dev-tools/');
      const connections = await page.evaluate(() => ({
        pickers: window.serviceGuideFixture.log.filter((entry) => entry[0] === 'picker').length,
        opens: window.serviceGuideFixture.log.filter((entry) => entry[0] === 'open').length,
      }));
      assert.deepEqual(connections, { pickers: 2, opens: 1 });
    } else {
      assert.equal(await connect.isDisabled(), true);
      assert.match(await guide.innerText(), /Use Chrome or Edge/);
    }
    const catalog = validateCatalog(
      (await (
        await page.request.get(new URL('runtime/catalog.json', base).href)
      ).json()) as Catalog,
    );
    for (const id of ['x70-110', 'xt1-552']) {
      const firmware = catalog.firmwares.find((entry) => entry.id === id);
      assert.ok(firmware?.service_workflow, `${id}: generated service workflow`);
      const response: APIResponse = await page.request.get(
        new URL(firmware.service_workflow, base).href,
      );
      assert.equal(response.ok(), true);
      const workflow = (await response.json()) as {
        firmware_id: string;
        firmware_sha256: string;
        setup: { guard?: unknown; debug_offset?: number };
        persistence?: unknown;
      };
      assert.equal(workflow.firmware_id, firmware.id);
      assert.equal(workflow.firmware_sha256, firmware.sha256);
      assert.ok(workflow.setup.guard);
      assert.equal(typeof workflow.setup.debug_offset, 'number');
      assert.ok(workflow.persistence);
    }
    assert.deepEqual(errors, []);
    console.log(
      `${browserType.name()}: service guide deep link, inputs and service workflow asset passed`,
    );
  } finally {
    await browser.close();
  }
}
