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
      await readFile(new URL(`./${name}_contract.py`, import.meta.url), 'utf8'),
    );
  }
  py.FS.writeFile(
    `${py.FS.cwd()}/contract_assertions.py`,
    await readFile(new URL('./contract_assertions.py', import.meta.url), 'utf8'),
  );
  const result = await py.runPythonAsync(
    (await readFile(new URL('./webusb_contract.py', import.meta.url), 'utf8')) + '\nawait run()',
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
