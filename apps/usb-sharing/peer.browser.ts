import '../../crates/usb-proxy/web/browser-state.ts';
import type { PeerChannel } from '../../crates/usb-proxy/web/peer.ts';
// Browser + native FujiMod clients through the actual local Workers runtime.
// Optional local TURN forces relay transport without Cloudflare credentials.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { runtime } from './test-runtime.ts';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { chromium } =
  require('playwright') as typeof import('../web/node_modules/playwright/index.js');
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL to the named HTTPS development app');
const repo = new URL('../../', import.meta.url).pathname;
const relay = process.env.TURN_TEST_URL;
const ice = relay
  ? [
      {
        urls: [relay],
        username: process.env.TURN_TEST_USER,
        credential: process.env.TURN_TEST_PASSWORD,
      },
    ]
  : undefined;
if (relay && (!ice?.[0].username || !ice?.[0].credential))
  throw Error('Set TURN_TEST_USER and TURN_TEST_PASSWORD');
const mf = runtime({
  bindings: {
    ALLOWED_ORIGINS: new URL(base).origin,
    ...(relay ? { TURN_KEY_ID: 'test-key', TURN_API_TOKEN: 'test-master-secret' } : {}),
  },
  ...(relay
    ? {
        outboundService: async (request) =>
          request.url.endsWith('/revoke')
            ? new Response(null, { status: 204 })
            : Response.json({ iceServers: ice }),
      }
    : {}),
});
const browser = await chromium.launch({ headless: true });
let debugTimer: ReturnType<typeof setInterval> | undefined;
try {
  const address = new URL('/usb-proxy/signal', await mf.ready);
  address.protocol = 'ws:';
  const context = await browser.newContext();
  await context.grantPermissions(['local-network-access'], { origin: new URL(base).origin });
  await context.routeWebSocket(
    (url) => url.searchParams.has('token'),
    (socket) => socket.close(),
  );
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.goto(new URL('usb-proxy/', base).href);
  if (process.env.FUJIMOD_TEST_TRACE)
    debugTimer = setInterval(() => {
      void page
        .evaluate(async () => ({
          logs: globalThis.workerPeerTest.logs,
          peers: await Promise.all(
            [...globalThis.workerPeerTest.host.peers.values()]
              .filter((peer): peer is PeerChannel => 'pc' in peer)
              .map(async (peer) => ({
                parts: peer.parts.length,
                bytes: peer.size,
                buffered: peer.channel.bufferedAmount,
                transports: [...(await peer.pc.getStats()).values()].filter(
                  (stat) => stat.type === 'transport' || stat.type === 'candidate-pair',
                ),
              })),
          ),
        }))
        .then((stats) => console.error('Peer stats', JSON.stringify(stats)))
        .catch(() => {});
    }, 5000);
  const link = await page.evaluate(
    async ({ repo, base, signal, relay }) => {
      if (relay) {
        const Original = RTCPeerConnection;
        globalThis.RTCPeerConnection = class extends Original {
          constructor(configuration?: RTCConfiguration) {
            super({ ...configuration, iceTransportPolicy: 'relay' });
          }
        };
      }
      const { installWebUsb } = (await import(
        '/@fs/' + repo + 'bindings/javascript/tests/webusb-fixture.ts'
      )) as typeof import('../../bindings/javascript/tests/webusb-fixture.ts');
      const fixture = installWebUsb();
      fixture.iface.alternates[0].endpoints = [
        { endpointNumber: 2, direction: 'in', type: 'bulk', packetSize: 64 },
        { endpointNumber: 1, direction: 'out', type: 'bulk', packetSize: 64 },
      ];
      fixture.iface.alternate = fixture.iface.alternates[0];
      const { ShareHost, newTicket, shareUrl } = (await import(
        '/@fs/' + repo + 'crates/usb-proxy/web/peer.ts'
      )) as typeof import('../../crates/usb-proxy/web/peer.ts');
      const { initialize, usb } = (await import(
        '/@fs/' + repo + 'bindings/javascript/dist/index.js'
      )) as typeof import('../../bindings/javascript/dist/index.js');
      await initialize();
      const [device] = await usb.discover(usb.DeviceFilter.Diagnostic);
      const ticket = newTicket(signal, []),
        logs: string[] = [];
      const host = await ShareHost.create(ticket, device.share(), {
        connected: () => logs.push('connected'),
        disconnected: () => logs.push('disconnected'),
        error: (error) => logs.push(error.message),
      });
      globalThis.workerPeerTest = { host, fixture, logs } as typeof workerPeerTest;
      return shareUrl(new URL('dev-tools/', base).href, ticket);
    },
    { repo, base, signal: address.href, relay: Boolean(relay) },
  );
  const setup = String.raw`
from fujimod import usb
from fujimod.usb import blocking
devices=blocking.discover(usb.DeviceFilter.Diagnostic)
assert len(devices)==1
choices=blocking.interfaces(devices[0])
limits=usb.FirmwareLimits(memory_read=256,memory_write=256,configuration=256,console=4096)
`;
  async function native(source: string) {
    const env: NodeJS.ProcessEnv = { ...process.env, FUJIMOD_USB_PROXY_URL: link };
    delete env.FUJIMOD_USB_PROXY_TOKEN;
    const child = spawn(process.env.PYTHON || 'python3', ['-c', setup + source], {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let out = '',
      error = '';
    child.stdout.on('data', (data) => {
      out += data;
      if (process.env.FUJIMOD_TEST_TRACE) console.error('Native', String(data));
    });
    child.stderr.on('data', (data) => {
      error += data;
      if (process.env.FUJIMOD_TEST_TRACE) console.error('Native trace', String(data));
    });
    const code = await new Promise((resolve, reject) => {
      child.on('exit', resolve);
      child.on('error', reject);
    });
    assert.equal(code, 0, error);
    return out;
  }
  await native(String.raw`
session=blocking.Session.open(devices[0],choices[0],limits,timeout_ms=20000)
assert session.device_info(usb.InfoSelector.Product)=='SYNTHETIC'
print('identified',flush=True)
payload=bytes(range(256))*8192
session.write_data(payload)
print('uploaded',flush=True)
assert session.read_data(0,len(payload))==payload
session.close()
`);
  await page.evaluate(
    async ({ repo, link, relay }) => {
      const { initialize, usb } = (await import(
        '/@fs/' + repo + 'bindings/javascript/dist/index.js'
      )) as typeof import('../../bindings/javascript/src/index.ts');
      await initialize();
      const devices = await usb.discoverProxy(link, '', usb.DeviceFilter.Diagnostic),
        choices = await devices[0].interfaces();
      const session = await usb.Session.open(
        devices[0],
        choices[0],
        new usb.FirmwareLimits(256, 256, 256, 4096),
        new usb.TransferOptions(20000, 8388608),
      );
      if ((await session.deviceInfo(usb.InfoSelector.Product)).text !== 'SYNTHETIC')
        throw Error('Wrong USB device');
      workerPeerTest.session = session;
      if (relay) {
        const owner = [...workerPeerTest.host.peers.values()].find(
          (peer): peer is PeerChannel =>
            'pc' in peer && (peer as PeerChannel).pc.connectionState === 'connected',
        );
        const stats = await owner!.pc.getStats();
        const transport = [...stats.values()].find(
          (stat) => stat.type === 'transport' && stat.selectedCandidatePairId,
        );
        const pair = stats.get(transport!.selectedCandidatePairId),
          local = stats.get(pair.localCandidateId);
        if (local.candidateType !== 'relay') throw Error('TURN was not used');
      }
    },
    { repo, link, relay: Boolean(relay) },
  );
  await native(String.raw`
try: blocking.Session.open(devices[0],choices[0],limits)
except usb.UsbError as error: assert 'in use' in str(error),str(error)
else: raise AssertionError('Second controller acquired the device')
`);
  await page.evaluate(async () => {
    await workerPeerTest.session.close();
    workerPeerTest.host.stop();
  });
  assert.deepEqual(errors, []);
  console.log(
    `PASS Worker rendezvous, native 2 MiB transfers, browser controller and ownership${relay ? ', forced TURN relay verified in WebRTC stats' : ''}`,
  );
} finally {
  clearInterval(debugTimer);
  await browser.close();
  await mf.dispose();
}
