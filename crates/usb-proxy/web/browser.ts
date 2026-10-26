import './browser-state.ts';
import { stripTypeScriptTypes } from 'node:module';
// Runs the real served exporter, relay and native/Wasm/Pyodide controllers.
// WebUSB is synthetic in an isolated context. No physical USB access.
import { createRequire } from 'node:module';
import { readFile, mkdir } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import assert from 'node:assert/strict';
const require = createRequire(
  new URL('../../../bindings/javascript/package.json', import.meta.url),
);
const { chromium } =
  require('playwright') as typeof import('../../../bindings/javascript/node_modules/playwright/index.js');
const root = new URL('../../../', import.meta.url);
const url = process.env.USB_PROXY_TEST_URL;
const token = process.env.FUJIMOD_USB_PROXY_TOKEN;
if (!url || !token) throw Error('Set USB_PROXY_TEST_URL and FUJIMOD_USB_PROXY_TOKEN');
const ws = new URL('/usb-proxy/ws', url);
ws.protocol = ws.protocol === 'https:' ? 'wss:' : 'ws:';
const fixtureSource = stripTypeScriptTypes(
  await readFile(
    new URL('../../../bindings/javascript/tests/webusb-fixture.ts', import.meta.url),
    'utf8',
  ),
);
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
    : {}),
});
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(
    fixtureSource.replace('export function installWebUsb', 'function installWebUsb') +
      `
    const f = installWebUsb(); globalThis.proxyFixture = f;
    f.iface.alternates[0].endpoints = [
      {endpointNumber:2,direction:'in',type:'bulk',packetSize:64},
      {endpointNumber:1,direction:'out',type:'bulk',packetSize:64}];
    f.iface.alternate = f.iface.alternates[0];
    f.device.close = async () => { f.device.opened = false; f.iface.claimed = false; f.log.push(['host-close']); };
    navigator.usb.getDevices = async () => [f.device]; navigator.usb.addEventListener = () => {};
  `,
  );
  await page.goto(url);
  assert.equal(await page.title(), 'USB proxy · FujiMod');
  await page.getByLabel('Relay token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await page.getByRole('cell', { name: 'Synthetic service device', exact: true }).waitFor();
  assert.match((await page.locator('#status').textContent())!, /^Connected/);
  const output = new URL('../../../target/usb-proxy-browser/', import.meta.url);
  await mkdir(output, { recursive: true });
  await page.screenshot({ path: new URL('exporter.png', output).pathname });
  const python =
    process.env.USB_PROXY_TEST_PYTHON ||
    new URL('../../../../.venv/bin/python', import.meta.url).pathname;
  for (const mode of ['discovery', 'async']) {
    const child = spawn(python, [new URL('./controller.py', import.meta.url).pathname, mode], {
      env: { ...process.env, FUJIMOD_USB_PROXY_URL: ws.href },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '',
      stderr = '';
    child.stdout.on('data', (data) => {
      stdout += data;
    });
    child.stderr.on('data', (data) => {
      stderr += data;
    });
    const code = await new Promise((resolve, reject) => {
      child.once('close', resolve);
      child.once('error', reject);
    });
    assert.equal(code, 0, stderr);
    assert.equal(JSON.parse(stdout).passed, true);
  }
  // Serve the freshly built binding and Python sources only inside this isolated
  // controller context. The actual exporter and WSS relay use the shared URL.
  const controller = await browser.newContext();
  await controller.grantPermissions(['local-network-access'], { origin: new URL(url).origin });
  const assets = new Map<string, [string, string, boolean?]>([
    [
      '/proxy-test/index.html',
      ['<!doctype html><title>Proxy controller test</title>', 'text/html', true],
    ],
    ['/dist/index.js', ['bindings/javascript/dist/index.js', 'text/javascript']],
    ['/pkg/fujimod.js', ['bindings/javascript/pkg/fujimod.js', 'text/javascript']],
    ['/pkg/service-values.js', ['bindings/javascript/pkg/service-values.js', 'text/javascript']],
    ['/pkg/fujimod_bg.wasm', ['bindings/javascript/pkg/fujimod_bg.wasm', 'application/wasm']],
  ]);
  for (const name of [
    'pyodide.mjs',
    'pyodide.asm.mjs',
    'pyodide.asm.wasm',
    'python_stdlib.zip',
    'pyodide-lock.json',
  ])
    assets.set('/pyodide/' + name, [
      'bindings/javascript/node_modules/pyodide/' + name,
      name.endsWith('.js') || name.endsWith('.mjs')
        ? 'text/javascript'
        : name.endsWith('.wasm')
          ? 'application/wasm'
          : 'application/octet-stream',
    ]);
  await controller.route('**/*', async (route) => {
    const path = new URL(route.request().url()).pathname;
    const entry =
      assets.get(path) ??
      (path.startsWith('/pkg/snippets/') && path.endsWith('.js')
        ? ['bindings/javascript' + path, 'text/javascript']
        : undefined);
    if (!entry) return route.continue();
    await route.fulfill({
      status: 200,
      contentType: entry[1],
      body: entry[2] ? entry[0] : await readFile(new URL(entry[0], root)),
    });
  });
  const cp = await controller.newPage();
  cp.setDefaultTimeout(120000);
  cp.on('pageerror', (error) => errors.push(String(error)));
  cp.on('console', (message) => {
    if (message.type() === 'error') console.error('Controller browser:', message.text());
  });
  await cp.goto(new URL('/proxy-test/index.html', url).href);
  const js = await cp.evaluate(
    async ({ ws, token }) => {
      const { initialize, usb } = (await import(
        '/dist/' + 'index.js'
      )) as typeof import('../../../bindings/javascript/src/index.ts');
      await initialize();
      const devices = await usb.discoverProxy(ws, token, usb.DeviceFilter.Diagnostic);
      if (devices.length !== 1) throw Error('Missing remote device');
      const choices = await devices[0].interfaces();
      const session = await usb.Session.open(
        devices[0],
        choices[0],
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(5000, 8192),
      );
      const product = (await session.deviceInfo(usb.InfoSelector.Product)).text;
      const data = [...(await session.readData(123, 5))];
      await session.close();
      return { product, data };
    },
    { ws: ws.href, token },
  );
  assert.deepEqual(js, { product: 'SYNTHETIC', data: [123, 124, 125, 126, 127] });
  // Install public Python source directly into Pyodide's FS to exercise this
  // exact checkout, without a stale generated wheel or a second codec.
  const paths: Record<string, string> = JSON.parse(
    (
      await promisify(execFile)('python3', [
        '-c',
        `import json,pathlib; p=pathlib.Path(${JSON.stringify(root.pathname + 'bindings/python/python')}); print(json.dumps({str(f.relative_to(p)):f.read_text() for f in p.rglob('*.py')}))`,
      ])
    ).stdout,
  );
  const contract = await readFile(new URL('./controller.py', import.meta.url), 'utf8');
  const pyodide = await cp.evaluate(
    async ({ paths, contract, ws, token }) => {
      const { loadPyodide } = (await import(
        '/pyodide/' + 'pyodide.mjs'
      )) as typeof import('../../../bindings/javascript/node_modules/pyodide/pyodide.js');
      const { registerPyodide } = (await import(
        '/dist/' + 'index.js'
      )) as typeof import('../../../bindings/javascript/src/index.ts');
      const py = await loadPyodide({ indexURL: '/pyodide/' });
      registerPyodide(py);
      for (const [path, text] of Object.entries(paths)) {
        py.FS.mkdirTree('/proxy-python/' + path.slice(0, path.lastIndexOf('/')));
        py.FS.writeFile('/proxy-python/' + path, text);
      }
      py.globals.set('relay_url', ws);
      py.globals.set('relay_token', token);
      // The shared contract's native-only imports and main do not apply here.
      const source = contract.split("if __name__ == '__main__':")[0];
      return await py.runPythonAsync(
        "import sys;sys.path.insert(0,'/proxy-python')\n" +
          source +
          '\nawait check_async(relay_url, relay_token)',
      );
    },
    { paths, contract, ws: ws.href, token },
  );
  assert.equal(pyodide, true);
  assert.deepEqual(errors, []);
  const closes = await page.evaluate(
    () => proxyFixture.log.filter((e) => e[0] === 'control' && e[1] === 0xc4).length,
  );
  await page.evaluate(() => {
    proxyFixture.state.hangControl = 0xc2;
  });
  const timed = await promisify(execFile)(
    python,
    [new URL('./controller.py', import.meta.url).pathname, 'timeout'],
    {
      env: { ...process.env, FUJIMOD_USB_PROXY_URL: ws.href },
    },
  );
  assert.equal(JSON.parse(timed.stdout).passed, true);
  assert.equal(
    await page.evaluate(
      () => proxyFixture.log.filter((e) => e[0] === 'control' && e[1] === 0xc4).length,
    ),
    closes,
  );
  await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
  console.log(
    'PASS real relay + browser exporter: native Python discovery/explicit proxy, Wasm and Pyodide controllers; synthetic USB identification, binary memory/data and tracing',
  );
} finally {
  await browser.close();
}
