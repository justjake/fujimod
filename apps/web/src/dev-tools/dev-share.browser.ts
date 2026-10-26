// Permitted synthetic WebUSB discovery over production Wasm and the installed relay.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, readdir, readFile, stat, writeFile } from 'node:fs/promises';
import { setTimeout } from 'node:timers/promises';
import { browserScript } from '../../scripts/browser-script.ts';
import { parseShare } from '../../../../crates/usb-proxy/web/peer.ts';

declare global {
  interface Window {
    devSharingFixture: ReturnType<typeof import('./usb-fixture.ts').installConsoleFixture>;
    releaseDevSharingDiscovery(): void;
  }
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base || !new URL(base).hostname.endsWith('.ts.net'))
  throw Error('Set FUJIMOD_WEB_URL to the named HTTPS preview');
const url = new URL('dev-tools/', base),
  run = crypto.randomUUID();
url.searchParams.set('dev-share-test', run);
const registry = new URL('../../../../target/usb-relay-service/dev-shares/', import.meta.url);
const output = new URL('../../build/dev-share-browser/', import.meta.url);
await mkdir(output, { recursive: true });
interface Registration {
  url: string;
  camera: { model: string | null; serial: string | null };
  status: { online: boolean; reason?: string };
}
async function files() {
  const found = await Promise.all(
    (await readdir(registry))
      .filter((name) => name.endsWith('.json'))
      .map(async (name) => {
        try {
          const entry: Registration = JSON.parse(await readFile(new URL(name, registry), 'utf8'));
          return new URL(entry.url).searchParams.get('dev-share-test') === run ? name : undefined;
        } catch (error) {
          if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return;
          throw error;
        }
      }),
  );
  return found.filter((name): name is string => Boolean(name));
}
const observed: number[] = [];
async function count(expected: number) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const own = await files();
    if (own.length === expected) {
      observed.push(own.length);
      return own;
    }
    await setTimeout(100);
  }
  assert.equal((await files()).length, expected, 'Own development registry entries');
  return files();
}
const upstream = await browserScript(
  new URL('../../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
);
const fixture = (await browserScript(new URL('./usb-fixture.ts', import.meta.url))).replace(
  '../../../../bindings/javascript/tests/webusb-fixture.ts',
  './upstream.mjs',
);
const browser = await chromium.launch({ headless: true });
const errors: string[] = [];
try {
  const context = await browser.newContext({ viewport: { width: 1300, height: 900 } });
  await context.grantPermissions(['local-network-access'], { origin: url.origin });
  await context.addInitScript(() => {
    let release!: () => void;
    const available = new Promise<void>((resolve) => {
      release = resolve;
    });
    window.releaseDevSharingDiscovery = release;
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
  });
  const owner = await context.newPage();
  owner.setDefaultTimeout(30000);
  owner.on('pageerror', (error) => errors.push(error.message));
  let acknowledgments = 0;
  const registryConnections: number[] = [];
  owner.on('websocket', (socket) => {
    if (new URL(socket.url()).pathname !== '/usb-proxy/dev-share') return;
    const index = registryConnections.push(0) - 1;
    socket.on('framereceived', ({ payload }) => {
      const reply: { type?: string } = JSON.parse(String(payload));
      if (reply.type === 'registered') {
        acknowledgments++;
        registryConnections[index]++;
      }
    });
  });
  await owner.route('**/__test__/upstream.mjs', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: upstream }),
  );
  await owner.route('**/__test__/fixture.mjs', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: fixture }),
  );
  await owner.goto(url.href);
  await owner.evaluate(async () => {
    window.devSharingFixture = (
      (await import('/__test__/' + 'fixture.mjs')) as typeof import('./usb-fixture.ts')
    ).installConsoleFixture();
    window.devSharingFixture.state.interfaceDelay = 20;
    window.releaseDevSharingDiscovery();
  });
  await owner.locator('.wb-camera h2').filter({ hasText: 'SYNTHETIC' }).waitFor();
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /^Idle$/ })
    .waitFor();
  const log = owner.getByRole('region', { name: 'Connection log' });
  await log
    .locator('.cl-row')
    .filter({ hasText: 'Development share registered' })
    .first()
    .waitFor();
  const sharing = owner
    .locator('.wb-camera')
    .getByRole('group', { name: 'Sharing' })
    .getByRole('button', { name: 'On', exact: true });
  assert.equal(
    await sharing.getAttribute('aria-pressed').then((pressed) => pressed === 'true'),
    true,
  );
  assert.equal(
    await owner.evaluate(
      () => window.devSharingFixture.log.filter((e) => e[0] === 'picker').length,
    ),
    0,
    'Permitted cameras auto-share without using the picker',
  );
  const [file] = await count(1);
  assert.equal((await stat(new URL(file, registry))).mode & 0o777, 0o600);
  const first: Registration = JSON.parse(await readFile(new URL(file, registry), 'utf8'));
  const ticket = parseShare(first.url);
  assert.equal(new URL(first.url).origin, url.origin);
  assert.deepEqual(first.camera, { model: 'SYNTHETIC', serial: 'SYNTHETIC' });
  assert.equal(first.status.online, true);
  await owner.screenshot({ path: new URL('auto-shared.png', output).pathname, fullPage: true });
  // The fixture's hotplug events drive nusb presence; an offline share stays registered.
  await owner.evaluate(() => window.devSharingFixture.presence(false));
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /Unplugged/ })
    .waitFor();
  const offlineDeadline = Date.now() + 15000;
  let offline: Registration = first;
  do {
    offline = JSON.parse(await readFile(new URL(file, registry), 'utf8'));
    if (!offline.status.online) break;
    await setTimeout(100);
  } while (Date.now() < offlineDeadline);
  assert.equal(offline.status.online, false);
  assert.equal(offline.status.reason, 'unplugged');
  await count(1);
  await owner.evaluate(() => window.devSharingFixture.presence(true));
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /^Idle$/ })
    .waitFor();
  assert.equal(
    await sharing.getAttribute('aria-pressed').then((pressed) => pressed === 'true'),
    true,
  );
  await owner
    .locator('.wb-camera')
    .getByRole('group', { name: 'Sharing' })
    .getByRole('button', { name: 'Off', exact: true })
    .click();
  await count(0);
  await owner.getByRole('button', { name: 'Copy link', exact: true }).waitFor({ state: 'hidden' });
  await owner.getByRole('form', { name: 'Read memory', exact: true }).waitFor({ state: 'hidden' });
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /^Idle$/ })
    .waitFor();
  await setTimeout(300);
  await count(0);
  assert.equal(
    await sharing.getAttribute('aria-pressed').then((pressed) => pressed === 'true'),
    false,
    'Stopping remains off while connected',
  );
  // Reconnecting resets per-camera auto-sharing, creating a fresh ticket.
  await owner.evaluate(() => window.devSharingFixture.presence(false));
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /Unplugged/ })
    .waitFor();
  await owner.evaluate(() => window.devSharingFixture.presence(true));
  await owner
    .locator('.wb-camera .wb-state')
    .filter({ hasText: /^Idle$/ })
    .waitFor();
  const [nextFile] = await count(1);
  const next: Registration = JSON.parse(await readFile(new URL(nextFile, registry), 'utf8'));
  const fresh = parseShare(next.url);
  assert.notEqual(ticket.room, fresh.room);
  assert.notEqual(ticket.key, fresh.key);
  await owner.screenshot({
    path: new URL('reconnected-auto-share.png', output).pathname,
    fullPage: true,
  });
  await owner.close();
  await count(0);
  assert.deepEqual(
    registryConnections.map((count) => count > 0),
    [true, true],
  );
  assert.deepEqual(errors, []);
  await writeFile(
    new URL('result.json', output),
    JSON.stringify(
      {
        passed: true,
        url: url.href,
        acknowledgments,
        registryCounts: observed,
        permittedAutoShare: true,
        offlineRegistration: true,
        freshTickets: true,
        privateFiles: true,
      },
      null,
      2,
    ),
  );
  console.log(
    'PASS: permitted USB auto-share, private registration, offline/reconnected presence, stop stays off, reconnect creates fresh share, tab close withdraws registry',
  );
} catch (error) {
  for (const [index, page] of browser
    .contexts()
    .flatMap((context) => context.pages())
    .entries()) {
    await writeFile(
      new URL(`failure-${index}.txt`, output),
      await page.locator('body').innerText(),
    ).catch(() => {});
    await page
      .screenshot({ path: new URL(`failure-${index}.png`, output).pathname, fullPage: true })
      .catch(() => {});
  }
  throw error;
} finally {
  await browser.close();
  await count(0);
}
