import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { browserScript } from '../../scripts/browser-script.ts';

const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the named HTTPS preview');
const output = new URL('../../build/bulk-support/', import.meta.url);
await mkdir(output, { recursive: true });
const fixture = await browserScript(new URL('./usb-fixture.ts', import.meta.url));
const upstream = await browserScript(
  new URL('../../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
);
const browser = await chromium.launch({ headless: true });
try {
  for (const mobile of [false, true]) {
    const context = await browser.newContext({
      viewport: mobile ? { width: 390, height: 844 } : { width: 1300, height: 1000 },
      isMobile: mobile,
    });
    await context.addInitScript(() => {
      Object.defineProperty(navigator, 'usb', {
        configurable: true,
        value: Object.assign(new EventTarget(), {
          getDevices: async () => [],
          requestDevice() {
            throw Error('Fixture is not installed');
          },
        }),
      });
    });
    const page = await context.newPage();
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
    await page.goto(new URL('dev-tools/', base).href);
    await page.getByRole('heading', { name: 'Enter service mode', exact: true }).waitFor();
    await page.evaluate(async () => {
      const fixture = (await import(
        '/__test__/' + 'console.mjs'
      )) as typeof import('./usb-fixture.ts');
      const device = fixture.installConsoleFixture({
        1: 'FUJIFILM',
        2: 'FUJIFILM',
        4: 'X-E4',
        8: '1.43',
        16: 'CAMERA',
        64: '1.00',
      });
      device.state.configuration.set('0:247', 0);
    });
    await page
      .locator('.td-connects')
      .getByRole('button', { name: 'Connect camera', exact: true })
      .click();
    await page.locator('.wb-camera dl').waitFor();
    await page.locator('.wb-bulk-error').first().waitFor();
    assert.equal(await page.locator('.wb-bulk-error').count(), 3);
    for (const name of ['Back up flash', 'Dump memory', 'Enable debug mode']) {
      const control = page.getByRole('button', { name, exact: true });
      assert.ok(await control.isDisabled());
      const warning = control.locator('xpath=ancestor::li').locator('.wb-bulk-error');
      assert.match(await warning.innerText(), /No service-workflow locator for X-Processor 4/);
      const style = await warning.evaluate((element) => {
        const style = getComputedStyle(element);
        return {
          color: style.color,
          red: getComputedStyle(element).getPropertyValue('--wb-red').trim(),
          weight: style.fontWeight,
        };
      });
      assert.ok(Number(style.weight) >= 700);
      assert.notEqual(style.color, 'rgb(185, 185, 191)');
    }
    await page.goto(new URL('dev-tools/state-captures/', base).href);
    await page.evaluate(async () => {
      const fixture = (await import(
        '/__test__/' + 'console.mjs'
      )) as typeof import('./usb-fixture.ts');
      fixture.installConsoleFixture({
        1: 'FUJIFILM',
        2: 'FUJIFILM',
        4: 'X-E4',
        8: '1.43',
        16: 'CAMERA',
        64: '1.00',
      });
    });
    await page.getByRole('button', { name: 'Connect camera', exact: true }).first().click();
    await page.getByText('X-E4 2.01', { exact: false }).first().waitFor();
    assert.ok(
      await page.locator('.cap-row').first().getByRole('button', { name: 'Capture' }).isEnabled(),
    );
    await page.screenshot({
      path: new URL(mobile ? 'xe4-mobile.png' : 'xe4-desktop.png', output).pathname,
      fullPage: true,
    });
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log('PASS: X-E4 bulk capture is available while missing service setup stays blocked');
} finally {
  await browser.close();
}
