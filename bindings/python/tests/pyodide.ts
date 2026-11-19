// Exercises Python -> JS bindings -> Rust -> nusb -> fake WebUSB. No hardware.
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { initialize, registerPyodide } from '../../javascript/dist/index.js';
import { installWebUsb } from '../../javascript/tests/webusb-fixture.ts';
const require = createRequire(new URL('../../javascript/package.json', import.meta.url));
const {
  loadPyodide,
}: typeof import('../../javascript/node_modules/pyodide/pyodide.js') = require('pyodide');
await initialize({
  module_or_path: await readFile(new URL('../../javascript/pkg/fujimod_bg.wasm', import.meta.url)),
});
const fixture = installWebUsb();
try {
  const py = await loadPyodide({
    packageCacheDir: new URL('../../../target/pyodide-test-packages/', import.meta.url).pathname,
  });
  await py.loadPackage('micropip');
  registerPyodide(py);
  py.registerJsModule('_fixture', { state: fixture.state, log: fixture.log });
  const manifest = JSON.parse(
    await readFile(new URL('../../../target/web-wheels/manifest.json', import.meta.url), 'utf8'),
  );
  const wheel = await readFile(
    new URL('../../../target/web-wheels/' + manifest.wheel, import.meta.url),
  );
  py.FS.writeFile('/' + manifest.wheel, wheel);
  await py.runPythonAsync(
    `import micropip;await micropip.install(${JSON.stringify('emfs:/' + manifest.wheel)})`,
  );
  const testPackage = `${py.FS.cwd()}/bindings/python/tests`;
  py.FS.mkdirTree(testPackage);
  for (const path of ['bindings', 'bindings/python', 'bindings/python/tests']) {
    py.FS.writeFile(
      `${py.FS.cwd()}/${path}/__init__.py`,
      await readFile(new URL(`../../../${path}/__init__.py`, import.meta.url), 'utf8'),
    );
  }
  for (const name of [
    'exposure_contract',
    'extended_contract',
    'adjustment_contract',
    'lens_contract',
    'configuration_contract',
    'transfer_contract',
    'peripheral_contract',
    'media_contract',
    'network_contract',
    'contract_assertions',
    'webusb_contract',
  ]) {
    py.FS.writeFile(
      `${testPackage}/${name}.py`,
      await readFile(new URL(`./${name}.py`, import.meta.url), 'utf8'),
    );
  }
  const result = await py.runPythonAsync(
    'from bindings.python.tests.webusb_contract import run\nawait run()',
  );
  assert.equal(result, true);
  console.log(
    'PASS Pyodide production WebUSB: shared definitions, typed service variants/errors, USB commands, buffers, tracing, cancellation and completion fencing (mock USB)',
  );
} catch (error) {
  console.error(String(error));
  process.exitCode = 1;
} finally {
  fixture.restore();
}
