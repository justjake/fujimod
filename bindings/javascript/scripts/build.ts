import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
const root = fileURLToPath(new URL('../../..', import.meta.url));
const pkg = fileURLToPath(new URL('../pkg', import.meta.url));
const target = process.env.CARGO_TARGET_DIR
  ? resolve(root, process.env.CARGO_TARGET_DIR)
  : join(root, 'target');
function run(command: string, args: string[]) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
// The test package adds the synthetic USB endpoints of fujimod-javascript-testing.
const crate = process.argv.includes('--testing') ? 'fujimod-javascript-testing' : 'fujimod-javascript';
run('cargo', ['build', '-p', crate, '--profile', 'dist', '--target', 'wasm32-unknown-unknown']);
run(process.env.WASM_BINDGEN ?? join(root, 'target/tooling/bin/wasm-bindgen'), [
  join(target, `wasm32-unknown-unknown/dist/${crate.replaceAll('-', '_')}.wasm`),
  '--target',
  'web',
  '--out-dir',
  pkg,
  '--out-name',
  'fujimod',
]);

run('cargo', ['run', '-p', 'fujimod-bindgen', '--', '--out', pkg]);
