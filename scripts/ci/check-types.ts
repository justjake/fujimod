/** Check every maintained TypeScript entry point, including both Wasm ABIs. */
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
const root = fileURLToPath(new URL('../../', import.meta.url));
const compiler = resolve(root, 'node_modules/typescript/bin/tsc');
function run(args: string[], directory = root): void {
  const result = spawnSync(process.execPath, args, { cwd: directory, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
function check(config: string): void {
  run([compiler, '--noEmit', '-p', config]);
}
for (const config of [
  'tsconfig.json',
  'scripts/ci/tsconfig.json',
  'scripts/assets/tsconfig.json',
  'scripts/cloudflare/tsconfig.json',
  'crates/usb-proxy/web/tsconfig.json',
  'apps/usb-sharing/tsconfig.json',
  'crates/simulation/tsconfig.json',
  'crates/simulation/tools/tsconfig.json',
])
  check(config);
const bindings = resolve(root, 'bindings/javascript');
// The instrumented build exposes synthetic endpoints that production omits.
// Check its tests while those generated declarations are available.
run(['scripts/build.ts', '--testing'], bindings);
run([compiler], bindings);
check('crates/usb-proxy/web/tsconfig.tests.json');
check('apps/usb-sharing/tsconfig.tests.json');
check('apps/usb-sharing/tsconfig.browser.json');
for (const config of ['tsconfig.types.json', 'tsconfig.scripts.json', 'tsconfig.testing.json'])
  check(`bindings/javascript/${config}`);
run(['scripts/build.ts'], bindings);
run([compiler], bindings);
check('bindings/javascript/tsconfig.types.json');
check('bindings/javascript/tsconfig.scripts.json');
check('packages/emulator-ui/tsconfig.json');
check('apps/web/tsconfig.json');
