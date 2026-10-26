/** Lint the synthetic test ABI, then restore production generated assets. */
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
const root = fileURLToPath(new URL('../../', import.meta.url));
const bindings = resolve(root, 'bindings/javascript');
function run(command: string, args: string[], cwd = root): number {
  const result = spawnSync(command, args, { cwd, stdio: 'inherit' });
  if (result.error) throw result.error;
  return result.status ?? 1;
}
const compiler = resolve(root, 'node_modules/typescript/bin/tsc');
let status = run(process.execPath, ['scripts/build.ts', '--testing'], bindings);
try {
  if (status === 0) status = run(process.execPath, [compiler], bindings);
  if (status === 0)
    status = run(resolve(root, 'node_modules/.bin/vp'), [
      'lint',
      '--deny-warnings',
      'bindings',
      'crates',
      'apps/usb-sharing',
      'scripts',
      'vite.config.ts',
    ]);
} finally {
  const restored = run(process.execPath, ['scripts/build.ts'], bindings);
  const declarations = restored === 0 ? run(process.execPath, [compiler], bindings) : restored;
  if (status === 0) status = declarations;
}
process.exitCode = status;
