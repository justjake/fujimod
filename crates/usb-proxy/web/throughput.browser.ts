// Camera-free end-to-end throughput: Rust Client <-> WebRTC <-> ShareHost.
// Run from the workspace root; builds its own assets and isolated relay.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import type { AddressInfo } from 'node:net';
import type { UsbExporter } from './types.ts';

const root = new URL('../../../', import.meta.url);
const require = createRequire(new URL('bindings/javascript/package.json', root));
const { chromium } =
  require('playwright') as typeof import('../../../bindings/javascript/node_modules/playwright/index.js');
execFileSync(
  'cargo',
  [
    'build',
    '--release',
    '-p',
    'fujimod-usb-proxy',
    '--features',
    'relay',
    '--bin',
    'fujimod-usb-relay',
  ],
  { cwd: root, stdio: 'inherit' },
);
const relayPort = Number(process.env.USB_PROXY_BENCH_RELAY_PORT || 15202);
const relayEnv: NodeJS.ProcessEnv = {
  ...process.env,
  FUJIMOD_USB_PROXY_TOKEN: 'isolated-throughput-benchmark-token',
};
delete relayEnv.FUJIMOD_USB_PROXY_TOKEN_FILE;
delete relayEnv.FUJIMOD_USB_PROXY_DEV_SHARES;
const relay = spawn(
  new URL('target/release/fujimod-usb-relay', root).pathname,
  [`127.0.0.1:${relayPort}`],
  {
    cwd: root,
    env: relayEnv,
    stdio: ['ignore', 'pipe', 'pipe'],
  },
);
const relayReady = new Promise<void>((resolve, reject) => {
  const timer = setTimeout(() => reject(Error('Benchmark relay did not start')), 10000);
  const fail = (error: Error) => {
    clearTimeout(timer);
    reject(error);
  };
  relay.once('error', fail);
  relay.once('exit', (code) => fail(Error(`Relay exited: ${code}`)));
  relay.stdout.once('data', () => {
    clearTimeout(timer);
    resolve();
  });
  relay.stderr.on('data', (data: Buffer) => process.stderr.write(data));
});
void relayReady.catch(() => {});
// Only compiled transport modules are served. There is no USB picker or WebUSB API.
const server = createServer(async (request, response) => {
  try {
    if (request.url === '/') {
      response.setHeader('Content-Type', 'text/html');
      response.end('<!doctype html><title>USB transport benchmark</title>');
    } else if (/^\/[a-z-]+\.js$/.test(request.url || '')) {
      const data = await readFile(new URL(`build${request.url}`, import.meta.url));
      response.setHeader('Content-Type', 'text/javascript');
      response.end(data);
    } else {
      response.writeHead(404).end();
    }
  } catch {
    response.writeHead(404).end();
  }
});
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
try {
  await relayReady;
  browser = await chromium.launch({ headless: true });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const page = await browser.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.goto(origin);
  const link = await page.evaluate(async (relayPort) => {
    const { ShareHost, newTicket, shareUrl } =
      (await import('./peer.js')) as typeof import('./peer.ts');
    const { decodeMessage, encodeMessage } =
      (await import('./wire.js')) as typeof import('./wire.ts');
    const data = Uint8Array.from({ length: 8 * 1024 * 1024 }, (_, i) => i & 255);
    const device: UsbExporter = {
      inventoryJson: JSON.stringify([
        {
          id: 'throughput',
          vendor_id: 0,
          product_id: 0,
          manufacturer: null,
          product: 'Transport benchmark',
          serial: 'throughput-fixture',
          interfaces: [],
        },
      ]),
      busy: () => false,
      owns: () => true,
      fenced: () => false,
      abandon: () => false,
      stopSharing: () => {},
      shutdown: async () => {},
      free: () => {},
      async execute(_controller, bytes) {
        const message = decodeMessage(bytes);
        if (message.type !== 'execute') throw Error('Expected execute');
        const { op: operation, seq } = message;
        const run = async () => {
          if (operation.op === 'open' || operation.op === 'close') return {};
          if (operation.op === 'transfer_out') {
            const received = operation.data!;
            for (let i = 0; i < received.length; i++)
              if (received[i] !== (i & 255)) throw Error(`Upload corruption at ${i}`);
            return { transferred: received.length };
          }
          if (operation.op === 'transfer_in')
            return { data: data.subarray(0, operation.length), transferred: operation.length };
          throw Error('Unsupported benchmark operation');
        };
        return encodeMessage({ type: 'result', seq, completion: await run() });
      },
    };
    const ticket = newTicket(`ws://127.0.0.1:${relayPort}/usb-proxy/signal`, []);
    await ShareHost.create(ticket, device);
    return shareUrl(location.href, ticket);
  }, relayPort);
  const profile = process.env.USB_PROXY_BENCH_PROFILE || 'release';
  assert.ok(['release', 'dev'].includes(profile), 'Benchmark profile must be release or dev');
  const test = spawn(
    'cargo',
    [
      'test',
      '--profile',
      profile,
      '-p',
      'fujimod-usb-proxy',
      '--test',
      'throughput',
      '--',
      '--ignored',
      '--nocapture',
    ],
    {
      cwd: root,
      env: { ...process.env, FUJIMOD_USB_PROXY_TEST_LINK: link },
      stdio: ['ignore', 'pipe', 'inherit'],
    },
  );
  let output = '';
  test.stdout.on('data', (data: Buffer) => {
    output += data.toString();
    process.stdout.write(data);
  });
  const [code] = await once(test, 'exit');
  const results: unknown[] = output
    .split('\n')
    .filter((line) => line.startsWith('THROUGHPUT '))
    .map((line) => JSON.parse(line.slice('THROUGHPUT '.length)) as unknown);
  const artifacts = new URL('target/usb-proxy-throughput/', root);
  await mkdir(artifacts, { recursive: true });
  await writeFile(
    new URL('latest.json', artifacts),
    JSON.stringify(
      {
        at: new Date().toISOString(),
        profile,
        topology: 'same-host native/Chromium WebRTC',
        minimumMBps: Number(process.env.USB_PROXY_MIN_MBPS || 20),
        exitCode: code,
        results,
        errors,
      },
      null,
      2,
    ) + '\n',
  );
  assert.equal(code, 0, 'Throughput/integrity check failed');
  assert.deepEqual(errors, []);
} finally {
  await browser?.close();
  server.close();
  relay.kill();
}
