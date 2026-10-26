import './browser-state.ts';
// Synthetic USB over real browser<->Rust WebRTC, including SCTP chunking.
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { spawn, execFileSync } from 'node:child_process';
import assert from 'node:assert/strict';
const require = createRequire(
  new URL('../../../bindings/javascript/package.json', import.meta.url),
);
const { chromium } =
  require('playwright') as typeof import('../../../bindings/javascript/node_modules/playwright/index.js');
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL');
const repo = new URL('../../../', import.meta.url).pathname;
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext();
  await context.grantPermissions(['local-network-access'], { origin: new URL(base).origin });
  await context.routeWebSocket(
    (url) => url.searchParams.has('token'),
    (socket) => socket.close(),
  );
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(new URL('usb-proxy/', base).href);
  const link = await page.evaluate(
    async ({ repo, base }) => {
      const { installWebUsb } = (await import(
        '/@fs/' + repo + 'bindings/javascript/tests/webusb-fixture.ts'
      )) as typeof import('../../../bindings/javascript/tests/webusb-fixture.ts');
      const fixture = installWebUsb();
      fixture.iface.alternates[0].endpoints = [
        { endpointNumber: 2, direction: 'in', type: 'bulk', packetSize: 64 },
        { endpointNumber: 1, direction: 'out', type: 'bulk', packetSize: 64 },
      ];
      fixture.iface.alternate = fixture.iface.alternates[0];
      const { initialize, usb } = (await import(
        '/@fs/' + repo + 'bindings/javascript/dist/index.js'
      )) as typeof import('../../../bindings/javascript/dist/index.js');
      await initialize();
      const [device] = await usb.discover(usb.DeviceFilter.Diagnostic);
      const { ShareHost, newTicket, shareUrl } = (await import(
        '/@fs/' + repo + 'crates/usb-proxy/web/peer.ts'
      )) as typeof import('./peer.ts');
      const signal = new URL('usb-proxy/signal', base);
      signal.protocol = 'wss:';
      const ticket = newTicket(signal.href, []),
        logs: string[] = [];
      const host = await ShareHost.create(ticket, device.share(), {
        connected: () => logs.push('connected'),
        disconnected: () => logs.push('disconnected'),
        error: (e) => logs.push(e.message),
      });
      const link = shareUrl(new URL('dev-tools/', base).href, ticket);
      globalThis.peerTest = { host, logs, fixture, link } as typeof peerTest;
      return link;
    },
    { repo, base },
  );
  // Probe the real peer transport without claiming or accessing USB.
  const echo = await page.evaluate(async (repo) => {
    const { connectPeer, parseShare } = (await import(
      '/@fs/' + repo + 'crates/usb-proxy/web/peer.ts'
    )) as typeof import('./peer.ts');
    const { VERSION, encodeMessage, decodeMessage } = (await import(
      '/@fs/' + repo + 'crates/usb-proxy/web/wire.ts'
    )) as typeof import('./wire.ts');
    const before = JSON.stringify(peerTest.fixture.log);
    const peer = await connectPeer(peerTest.link);
    try {
      await peer.send(
        encodeMessage({
          type: 'hello',
          version: VERSION,
          role: 'controller',
          token: parseShare(peerTest.link).key,
        }),
      );
      const ready = decodeMessage(await peer.receive());
      if (ready.type !== 'ready') throw Error('Peer handshake failed');
      await peer.send(
        encodeMessage({ type: 'execute', seq: 1, device: '', op: { op: 'probe' }, timeout_ms: 10 }),
      );
      const reply = decodeMessage(await peer.receive());
      if (reply.type !== 'result') throw Error('Expected USB result');
      return { reply, untouched: before === JSON.stringify(peerTest.fixture.log) };
    } finally {
      peer.close();
    }
  }, repo);
  assert.equal(echo.untouched, true);
  assert.deepEqual(
    {
      ...echo.reply,
      completion: { ...echo.reply.completion, data: Object.values(echo.reply.completion.data!) },
    },
    { type: 'result', seq: 1, completion: { devices: [], data: [], transferred: 0, error: null } },
  );
  console.log('PASS USB-free latency echo over real WebRTC');
  // An idle native I/O loop must wake for the first outbound Hello. This
  // handshake uses no USB operations and must leave the exporter untouched.
  const beforeProbe = await page.evaluate(() => JSON.stringify(peerTest.fixture.log));
  const probe = spawn('cargo', ['run', '-p', 'fujimod-usb-proxy', '--bin', 'fujimod-share-probe'], {
    cwd: repo,
    env: { ...process.env, FUJIMOD_USB_PROXY_URL: link },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let probeOutput = '';
  probe.stdout.on('data', (data) => (probeOutput += data));
  probe.stderr.on('data', (data) => (probeOutput += data));
  const probeExit = await new Promise((resolve, reject) => {
    probe.on('exit', resolve);
    probe.on('error', reject);
  });
  assert.equal(probeExit, 0, probeOutput);
  assert.match(probeOutput, /BRIDGE READY/);
  assert.equal(await page.evaluate(() => JSON.stringify(peerTest.fixture.log)), beforeProbe);
  console.log('PASS idle native handshake without USB requests');
  // Exercise the actual native Rust client, with all byte values through the transfer limit.
  const rust = spawn(
    'cargo',
    ['test', '-p', 'fujimod-usb-proxy', '--test', 'browser', '--', '--ignored'],
    {
      cwd: repo,
      env: { ...process.env, FUJIMOD_USB_PROXY_TEST_LINK: link },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  let rustOutput = '';
  rust.stdout.on('data', (data) => {
    rustOutput += data;
  });
  rust.stderr.on('data', (data) => {
    rustOutput += data;
  });
  const rustExit = await new Promise((resolve, reject) => {
    rust.on('exit', resolve);
    rust.on('error', reject);
  });
  assert.equal(rustExit, 0, rustOutput);
  assert.deepEqual(
    await page.evaluate(() =>
      peerTest.fixture.state.pieces.splice(0).map((data) => ({
        length: data.length,
        valid: data.every((value, i) => value === (i & 255)),
      })),
    ),
    [1, 16000, 16001, 262145, 8388608].map((length) => ({ length, valid: true })),
  );
  console.log(
    'PASS native Rust binary WebRTC upload/download through 8 MiB, including framing boundaries',
  );
  const python =
    process.env.USB_PROXY_TEST_PYTHON ||
    new URL('../../../../.venv/bin/python', import.meta.url).pathname;
  async function native(script: string) {
    const env: NodeJS.ProcessEnv = { ...process.env, FUJIMOD_USB_PROXY_URL: link };
    delete env.FUJIMOD_USB_PROXY_TOKEN;
    const child = spawn(
      python,
      [
        '-c',
        'import asyncio\nasync def main():\n' +
          script
            .split('\n')
            .map((line) => '    ' + line)
            .join('\n') +
          '\nasyncio.run(main())',
      ],
      { env, stdio: ['ignore', 'pipe', 'pipe'] },
    );
    let out = '',
      err = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (err += d));
    const exit = await new Promise((resolve, reject) => {
      child.on('exit', resolve);
      child.on('error', reject);
    });
    assert.equal(exit, 0, err);
    return out;
  }
  const setup = String.raw`
from fujimod import usb
devices=await usb.discover(usb.DeviceFilter.Diagnostic)
assert len(devices)==1
choices=await usb.interfaces(devices[0])
limits=usb.FirmwareLimits(memory_read=256,memory_write=256,configuration=256,console=4096)
`;
  await native(
    setup +
      String.raw`
session=await usb.Session.open(devices[0],choices[0],limits,timeout_ms=20000)
assert await session.device_info(usb.InfoSelector.Product)=='SYNTHETIC'
data=bytes(range(256))*8192
await session.write_data(data)
assert await session.read_data(0,len(data))==data
await session.close()
print('native 2 MiB upload and download passed')
`,
  );
  assert.equal(await page.evaluate(() => peerTest.fixture.state.pieces[0].length), 2 * 1024 * 1024);
  console.log('PASS native WebRTC 2 MiB USB payloads in both directions');
  const pythonFiles = execFileSync('rg', ['--files', 'bindings/python/python', '-g', '*.py'], {
    cwd: repo,
    encoding: 'utf8',
  })
    .trim()
    .split('\n');
  const paths: Record<string, string> = Object.fromEntries(
    await Promise.all(
      pythonFiles.map(async (path) => [
        path.slice('bindings/python/python/'.length),
        await readFile(repo + path, 'utf8'),
      ]),
    ),
  );
  await page.evaluate(
    async ({ repo, paths }) => {
      const pyRoot = '/@fs/' + repo + 'bindings/javascript/node_modules/pyodide/';
      const { loadPyodide } = (await import(
        pyRoot + 'pyodide.mjs'
      )) as typeof import('../../../bindings/javascript/node_modules/pyodide/pyodide.js');
      const { registerPyodide, initialize } = (await import(
        '/@fs/' + repo + 'bindings/javascript/dist/index.js'
      )) as typeof import('../../../bindings/javascript/src/index.ts');
      const py = await loadPyodide({ indexURL: pyRoot });
      await initialize();
      registerPyodide(py);
      for (const [path, source] of Object.entries(paths)) {
        py.FS.mkdirTree('/proxy-python/' + path.slice(0, path.lastIndexOf('/')));
        py.FS.writeFile('/proxy-python/' + path, source);
      }
      py.globals.set('share_url', peerTest.link);
      await py.runPythonAsync(`
import sys
sys.path.insert(0,'/proxy-python')
from fujimod import usb
devices=await usb.discover_proxy(share_url,'',usb.DeviceFilter.Diagnostic)
choices=await usb.interfaces(devices[0])
limits=usb.FirmwareLimits(memory_read=256,memory_write=256,configuration=256,console=4096)
session=await usb.Session.open(devices[0],choices[0],limits)
assert await session.device_info(usb.InfoSelector.Product)=='SYNTHETIC'
assert await session.read_data(123,5)==bytes([123,124,125,126,127])
await session.close()
`);
    },
    { repo, paths },
  );
  console.log('PASS Pyodide controller using the same share URL');
  await page.evaluate(async (repo) => {
    const { initialize, usb } = (await import(
      '/@fs/' + repo + 'bindings/javascript/dist/index.js'
    )) as typeof import('../../../bindings/javascript/src/index.ts');
    await initialize();
    const devices = await usb.discoverProxy(peerTest.link, '', usb.DeviceFilter.Diagnostic);
    const choices = await devices[0].interfaces();
    const session = await usb.Session.open(
      devices[0],
      choices[0],
      new usb.FirmwareLimits(256, 256, 256, 4096),
      new usb.TransferOptions(5000, 8388608),
    );
    const data = Uint8Array.from({ length: 2 * 1024 * 1024 }, (_, i) => i & 255);
    await session.writeData(data);
    const received = await session.readData(0, data.length);
    if (received.length !== data.length || !received.every((value, i) => value === data[i]))
      throw Error('Wasm binary payload mismatch');
    peerTest.session = session;
  }, repo);
  await native(
    setup +
      String.raw`
try:
 await usb.Session.open(devices[0],choices[0],limits)
except usb.UsbError as error:
 assert 'in use' in str(error),str(error)
else:
 raise AssertionError('second controller claimed the USB device')
`,
  );
  await page.evaluate(() => {
    peerTest.session.abandon();
    peerTest.session.free();
  });
  await page.waitForFunction(() => peerTest.logs.some((v) => v.includes('completion unknown')));
  await native(
    setup +
      String.raw`
try:
 await usb.Session.open(devices[0],choices[0],limits)
except usb.UsbError as error:
 assert 'completion unknown' in str(error),str(error)
else:
 raise AssertionError('abandoned USB lease allowed another controller')
`,
  );
  await page.evaluate(() => peerTest.host.stop());
  assert.deepEqual(errors, []);
  console.log('PASS single-controller ownership and fencing after an abandoned peer session');
} finally {
  await browser.close();
}
