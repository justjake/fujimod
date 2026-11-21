import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { cargoArtifacts, readLinkedCrates } from './linked-crates.ts';
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
const testing = process.argv.includes('--testing');
const crate = testing ? 'fujimod-javascript-testing' : 'fujimod-javascript';
const evidence = fileURLToPath(new URL('../.tools/webusb', import.meta.url));
const args = ['rustc', '--locked', '-p', crate, '--profile', 'release-web',
  '--target', 'wasm32-unknown-unknown'];
let linked: string[] | undefined;
if (testing) {
  run('cargo', args);
} else {
  // The website's SBOM describes the distributable module. Test builds must
  // leave its linked-crate evidence intact.
  mkdirSync(evidence, { recursive: true });
  const map = join(evidence, crate + '.map');
  const artifacts = cargoArtifacts(args, root, process.env, [`-Clink-arg=-Map=${map}`]);
  linked = readLinkedCrates(map, artifacts);
}
run(process.env.WASM_BINDGEN ?? join(root, 'target/tooling/bin/wasm-bindgen'), [
  join(target, `wasm32-unknown-unknown/release-web/${crate.replaceAll('-', '_')}.wasm`),
  '--target',
  'web',
  '--out-dir',
  pkg,
  '--out-name',
  'fujimod',
]);
if (linked) writeFileSync(join(evidence, 'linked-crates.json'), JSON.stringify(linked) + '\n');

run('cargo', ['run', '--release', '-p', 'fujimod-bindgen', '--', '--out', pkg]);
