import { stripTypeScriptTypes } from 'node:module';
import type { chromium as chromiumType } from '../../javascript/node_modules/playwright/index.js';
// Only allowlisted public test/runtime assets are served. USB is always fake.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import assert from 'node:assert/strict';
const require = createRequire(new URL('../../javascript/package.json', import.meta.url));

const { chromium }: { chromium: typeof chromiumType } = require('playwright');
const manifest = JSON.parse(
  await readFile(new URL('../../../target/web-wheels/manifest.json', import.meta.url), 'utf8'),
);
const lock = JSON.parse(
  await readFile(
    new URL('../../javascript/node_modules/pyodide/pyodide-lock.json', import.meta.url),
    'utf8',
  ),
);
const micropip = lock.packages.micropip.file_name;
const assets = new Map<string, [string, string]>([
  ['/dist/index.js', ['../../javascript/dist/index.js', 'text/javascript']],
  ['/pkg/fujimod.js', ['../../javascript/pkg/fujimod.js', 'text/javascript']],
  ['/pkg/service-values.js', ['../../javascript/pkg/service-values.js', 'text/javascript']],
  ['/pkg/fujimod_bg.wasm', ['../../javascript/pkg/fujimod_bg.wasm', 'application/wasm']],
  ['/fixture.mjs', ['../../javascript/tests/webusb-fixture.ts', 'text/javascript']],
  ['/contract-assertions.py', ['./contract_assertions.py', 'text/plain']],
  ['/contract.py', ['./webusb_contract.py', 'text/plain']],
  ['/media-contract.py', ['./media_contract.py', 'text/plain']],
  ['/exposure-contract.py', ['./exposure_contract.py', 'text/plain']],
  ['/extended-contract.py', ['./extended_contract.py', 'text/plain']],
  ['/adjustment-contract.py', ['./adjustment_contract.py', 'text/plain']],
  ['/lens-contract.py', ['./lens_contract.py', 'text/plain']],
  ['/configuration-contract.py', ['./configuration_contract.py', 'text/plain']],
  ['/transfer-contract.py', ['./transfer_contract.py', 'text/plain']],
  ['/network-contract.py', ['./network_contract.py', 'text/plain']],
  ['/peripheral-contract.py', ['./peripheral_contract.py', 'text/plain']],
  ['/fujimod.whl', ['../../../target/web-wheels/' + manifest.wheel, 'application/zip']],
]);
for (const name of [
  'pyodide.mjs',
  'pyodide.asm.mjs',
  'pyodide.asm.wasm',
  'python_stdlib.zip',
  'pyodide-lock.json',
]) {
  assets.set('/pyodide/' + name, [
    '../../javascript/node_modules/pyodide/' + name,
    name.endsWith('.wasm') ? 'application/wasm' : 'application/octet-stream',
  ]);
}
assets.set('/pyodide/' + micropip, [
  '../../../target/pyodide-test-packages/' + micropip,
  'application/zip',
]);
const server = createServer(async (req, res) => {
  const path = new URL(req.url ?? '/', 'http://localhost').pathname;
  if (path === '/') {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><title>FujiMod Pyodide tests</title>');
    return;
  }
  const entry =
    assets.get(path) ??
    (path.startsWith('/pkg/snippets/') && path.endsWith('.js')
      ? ['../../javascript/' + path.slice(1), 'text/javascript']
      : undefined);
  if (!entry) {
    res.writeHead(404).end();
    return;
  }
  try {
    const bytes = await readFile(new URL(entry[0], import.meta.url));
    res.writeHead(200, { 'content-type': path.endsWith('.mjs') ? 'text/javascript' : entry[1] });
    res.end(entry[0].endsWith('.ts') ? stripTypeScriptTypes(bytes.toString()) : bytes);
  } catch {
    res.writeHead(500).end();
  }
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const address = server.address();
assert.ok(address && typeof address === 'object');
let browser;
try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : {}),
  });
  const page = await browser.newPage();
  page.setDefaultTimeout(120000);
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto(`http://127.0.0.1:${address.port}/`);
  const result = await page.evaluate(async (wheelName) => {
    const { loadPyodide } = (await import(
      String('/pyodide/pyodide.mjs')
    )) as typeof import('../../javascript/node_modules/pyodide/pyodide.js');
    const { initialize, registerPyodide } = (await import(
      String('/dist/index.js')
    )) as typeof import('../../javascript/dist/index.js');
    const { installWebUsb } = (await import(
      String('/fixture.mjs')
    )) as typeof import('../../javascript/tests/webusb-fixture.ts');
    await initialize();
    const fixture = installWebUsb();
    try {
      const py = await loadPyodide({ indexURL: '/pyodide/' });
      registerPyodide(py);
      py.registerJsModule('_fixture', { state: fixture.state, log: fixture.log });
      const wheel = new Uint8Array(await (await fetch('/fujimod.whl')).arrayBuffer());
      py.FS.writeFile('/' + wheelName, wheel);
      await py.loadPackage('micropip');
      await py.runPythonAsync(
        `import micropip;await micropip.install(${JSON.stringify('emfs:/' + wheelName)})`,
      );
      for (const name of [
        'exposure',
        'extended',
        'adjustment',
        'lens',
        'configuration',
        'transfer',
        'peripheral',
        'media',
        'network',
      ]) {
        py.FS.writeFile(
          `${py.FS.cwd()}/${name}_contract.py`,
          await (await fetch(`/${name}-contract.py`)).text(),
        );
      }
      py.FS.writeFile(
        `${py.FS.cwd()}/contract_assertions.py`,
        await (await fetch('/contract-assertions.py')).text(),
      );
      const passed = await py.runPythonAsync(
        (await (await fetch('/contract.py')).text()) + '\nawait run()',
      );
      return { passed, secure: isSecureContext, realWindow: window instanceof Window };
    } finally {
      fixture.restore();
    }
  }, manifest.wheel);
  assert.deepEqual(result, { passed: true, secure: true, realWindow: true });
  assert.deepEqual(errors, []);
  console.log(
    'PASS Chromium + Pyodide wheel: shared definitions, typed service controls, real Window, buffers/enums/errors, async ranges, publication/task/chunk cancellation and completion fencing (fake USB)',
  );
} finally {
  await browser?.close();
  server.close();
  await once(server, 'close');
}
