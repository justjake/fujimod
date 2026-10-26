// Production Wasm/nusb and WebRTC over a synthetic WebUSB camera; no hardware.
import { chromium, type Page } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { browserScript } from '../../scripts/browser-script.ts';
import type { Completion, Message, Operation } from '../../../../crates/usb-proxy/web/types.ts';

declare global {
  interface Window {
    sharingFixture: ReturnType<typeof import('./usb-fixture.ts').installConsoleFixture>;
    releaseSharingDiscovery(): void;
    sharingSpoken: string[];
    sharingController: {
      request(op: Operation): Promise<Completion>;
      notify(message: Message): Promise<void>;
      close(): Promise<void>;
    };
  }
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base || !new URL(base).hostname.endsWith('.ts.net'))
  throw Error('Set FUJIMOD_WEB_URL to the named HTTPS preview');
const output = new URL('../../build/sharing-browser/', import.meta.url);
const repo = new URL('../../../../', import.meta.url).pathname;
await mkdir(output, { recursive: true });
const upstream = await browserScript(
  new URL('../../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
);
const fixture = (await browserScript(new URL('./usb-fixture.ts', import.meta.url))).replace(
  '../../../../bindings/javascript/tests/webusb-fixture.ts',
  './upstream.mjs',
);
const browser = await chromium.launch({ headless: true });
const errors: string[] = [];
async function ready(page: Page) {
  await page.locator('.wb-camera h2').filter({ hasText: 'SYNTHETIC' }).waitFor();
  await page
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /^Idle$/ })
    .waitFor();
  await page.getByRole('form', { name: 'Read memory', exact: true }).waitFor({ state: 'hidden' });
}
try {
  const context = await browser.newContext({ viewport: { width: 1300, height: 900 } });
  await context.grantPermissions(['clipboard-read', 'clipboard-write', 'local-network-access'], {
    origin: new URL(base).origin,
  });
  await context.addInitScript(() => {
    let release!: () => void;
    const available = new Promise<void>((resolve) => {
      release = resolve;
    });
    window.releaseSharingDiscovery = release;
    Object.defineProperty(navigator, 'usb', {
      configurable: true,
      value: Object.assign(new EventTarget(), {
        async getDevices() {
          await available;
          return (
            navigator as Navigator & { usb: { getDevices(): Promise<unknown[]> } }
          ).usb.getDevices();
        },
        requestDevice() {
          throw Error('Physical USB forbidden');
        },
      }),
    });
    window.sharingSpoken = [];
    Object.defineProperty(window, 'speechSynthesis', {
      configurable: true,
      value: {
        speak(utterance: SpeechSynthesisUtterance) {
          window.sharingSpoken.push(utterance.text);
        },
        cancel() {},
      },
    });
  });
  const owner = await context.newPage();
  owner.setDefaultTimeout(30000);
  owner.on('pageerror', (error) => errors.push(`owner: ${error.message}`));
  await owner.route('**/__test__/upstream.mjs', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: upstream }),
  );
  await owner.route('**/__test__/fixture.mjs', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: fixture }),
  );
  const url = new URL('dev-tools/', base);
  url.searchParams.set('sharing-test', crypto.randomUUID());
  await owner.goto(url.href);
  await owner.evaluate(async () => {
    window.sharingFixture = (
      (await import('/__test__/' + 'fixture.mjs')) as typeof import('./usb-fixture.ts')
    ).installConsoleFixture();
    window.sharingFixture.state.interfaceDelay = 20;
    window.releaseSharingDiscovery();
  });
  await ready(owner);
  await owner.getByRole('button', { name: 'Copy link', exact: true }).waitFor();
  await owner
    .locator('.wb-camera')
    .getByRole('group', { name: 'Sharing' })
    .getByRole('button', { name: 'Off', exact: true })
    .click();
  await owner.getByRole('button', { name: 'Copy link', exact: true }).waitFor({ state: 'hidden' });
  await owner
    .locator('.wb-camera')
    .getByRole('group', { name: 'Sharing' })
    .getByRole('button', { name: 'On', exact: true })
    .click();
  await owner.getByRole('button', { name: 'Copy link', exact: true }).click();
  await owner.getByRole('button', { name: 'Copied', exact: true }).waitFor();
  const link = await owner.evaluate(() => navigator.clipboard.readText());
  assert.equal(new URL(link).origin, url.origin);
  assert.ok(new URL(link).hash.startsWith('#?remotecontrol='));
  assert.equal(
    await owner
      .locator('.wb-camera')
      .getByRole('group', { name: 'Sharing' })
      .getByRole('button', { name: 'On', exact: true })
      .getAttribute('aria-pressed'),
    'true',
  );
  await owner.screenshot({
    path: new URL('shared-local-camera.png', output).pathname,
    fullPage: true,
  });

  const remoteContext = await browser.newContext({ viewport: { width: 1300, height: 900 } });
  await remoteContext.grantPermissions(['local-network-access'], { origin: url.origin });
  await remoteContext.addInitScript(() =>
    Object.defineProperty(navigator, 'usb', {
      configurable: true,
      value: Object.assign(new EventTarget(), {
        async getDevices() {
          return [];
        },
        requestDevice() {
          throw Error('Physical USB forbidden');
        },
      }),
    }),
  );
  const remote = await remoteContext.newPage();
  remote.setDefaultTimeout(30000);
  remote.on('pageerror', (error) => errors.push(`remote: ${error.message}`));
  await remote.goto(link);
  await ready(remote);
  assert.match(await remote.locator('.wb-cam.selected').innerText(), /Remote · Idle/);
  assert.equal(
    await remote.locator('.wb-camera').getByRole('group', { name: 'Sharing' }).count(),
    0,
  );
  assert.match(await remote.locator('.wb-camera').innerText(), /Serial\s+SYNTHETIC/);
  await remote
    .getByRole('region', { name: 'Connection log' })
    .locator('.cl-row')
    .filter({ hasText: 'Device info · Product → SYNTHETIC' })
    .first()
    .waitFor();
  await remote.screenshot({
    path: new URL('remote-camera.png', output).pathname,
    fullPage: true,
  });

  await remote.evaluate(
    async ({ repo, link }) => {
      const { connectPeer, parseShare } = (await import(
        '/@fs/' + repo + 'crates/usb-proxy/web/peer.ts'
      )) as typeof import('../../../../crates/usb-proxy/web/peer.ts');
      const { VERSION, encodeMessage, decodeMessage } = (await import(
        '/@fs/' + repo + 'crates/usb-proxy/web/wire.ts'
      )) as typeof import('../../../../crates/usb-proxy/web/wire.ts');
      const peer = await connectPeer(link);
      let seq = 0,
        device = '';
      await peer.send(
        encodeMessage({
          type: 'hello',
          version: VERSION,
          role: 'controller',
          token: parseShare(link).key,
          reason: 'Sharing browser regression',
        }),
      );
      const handshake = decodeMessage(await peer.receive());
      if (handshake.type !== 'ready') throw Error('Controller handshake failed');
      window.sharingController = {
        async request(op) {
          const sent = ++seq;
          await peer.send(
            encodeMessage({ type: 'execute', seq: sent, device, timeout_ms: 30000, op }),
          );
          const reply = decodeMessage(await peer.receive());
          if (reply.type !== 'result' || reply.seq !== sent) throw Error('Unexpected proxy result');
          if (reply.completion.error) throw Error(reply.completion.error);
          return reply.completion;
        },
        notify: (message) => peer.send(encodeMessage(message)),
        async close() {
          await peer.send(encodeMessage({ type: 'goodbye', reason: 'done' }));
          peer.close();
        },
      };
      const inventory = await window.sharingController.request({ op: 'list' });
      device = inventory.devices![0].id;
      await window.sharingController.request({
        op: 'open',
        configuration: 1,
        interface: 2,
        alternate: 3,
      });
      await window.sharingController.notify({
        type: 'activity',
        title: 'Inspecting shared camera',
      });
      await window.sharingController.notify({
        type: 'progress_start',
        label: 'Inspection',
        total: 8,
      });
      await window.sharingController.notify({ type: 'progress_update', done: 3 });
    },
    { repo, link },
  );
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: 'Inspecting shared camera' })
    .waitFor();
  const progress = owner.getByRole('progressbar', { name: 'Inspection', exact: true });
  await progress.waitFor();
  assert.equal(await progress.getAttribute('aria-valuenow'), '3');
  await owner.getByRole('form', { name: 'Read memory', exact: true }).waitFor({ state: 'hidden' });
  let finished = false;
  const started = Date.now();
  const countdown = remote
    .evaluate(async () => {
      await window.sharingController.request({
        op: 'owner_attention',
        message: 'Watch the camera now',
      });
    })
    .then(() => {
      finished = true;
    });
  await owner.locator('.wb-countdown').filter({ hasText: 'Watch the camera now' }).waitFor();
  assert.equal(finished, false);
  await owner
    .locator('.wb-countdown strong')
    .filter({ hasText: /^[1-9]$/ })
    .waitFor();
  assert.equal(finished, false);
  await owner.screenshot({
    path: new URL('remote-countdown.png', output).pathname,
    fullPage: true,
  });
  await countdown;
  assert.ok(Date.now() - started >= 9000, 'remote waits for the spoken ten-second countdown');
  assert.deepEqual(await owner.evaluate(() => window.sharingSpoken), [
    '10',
    '9',
    '8',
    '7',
    '6',
    '5',
    '4',
    '3',
    '2',
    '1',
  ]);
  await owner.locator('.wb-countdown').waitFor({ state: 'hidden' });
  await remote.evaluate(async () => {
    await window.sharingController.notify({ type: 'progress_done' });
    await window.sharingController.notify({ type: 'activity', title: '' });
    await window.sharingController.request({ op: 'close' });
    await window.sharingController.close();
  });
  await ready(owner);
  await progress.waitFor({ state: 'hidden' });
  await owner
    .getByRole('region', { name: 'Connection log' })
    .locator('.cl-row')
    .filter({ hasText: 'Remote controller connected' })
    .first()
    .waitFor();
  await owner
    .locator('.wb-camera')
    .getByRole('group', { name: 'Sharing' })
    .getByRole('button', { name: 'Off', exact: true })
    .click();
  await owner.getByRole('button', { name: 'Copy link', exact: true }).waitFor({ state: 'hidden' });
  const usb = await owner.evaluate(() => ({
    open: window.sharingFixture.log.filter((e) => e[0] === 'open').length,
    claim: window.sharingFixture.log.filter((e) => e[0] === 'claim').length,
    closing: window.sharingFixture.log.filter((e) => ['release', 'close', 'refused'].includes(e[0]))
      .length,
  }));
  assert.deepEqual(usb, { open: 1, claim: 1, closing: 0 });
  assert.deepEqual(errors, []);
  await owner.screenshot({
    path: new URL('stopped-local-camera.png', output).pathname,
    fullPage: true,
  });
  await writeFile(
    new URL('result.json', output),
    JSON.stringify(
      {
        passed: true,
        url: url.href,
        remoteIdentity: true,
        sharedLocalRead: true,
        stoppedLocalRead: true,
        countdownWaited: true,
        activity: true,
        usb,
      },
      null,
      2,
    ),
  );
  console.log(
    'PASS: sharing switch, clipboard, remote identity/read, host reads while shared and stopped, activity/progress, awaited countdown, one USB owner',
  );
} catch (error) {
  for (const [i, page] of browser
    .contexts()
    .flatMap((context) => context.pages())
    .entries()) {
    await writeFile(
      new URL(`failure-${i}.txt`, output),
      await page.locator('body').innerText(),
    ).catch(() => {});
    await page
      .screenshot({ path: new URL(`failure-${i}.png`, output).pathname, fullPage: true })
      .catch(() => {});
  }
  throw error;
} finally {
  await browser.close();
}
