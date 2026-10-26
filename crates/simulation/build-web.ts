import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  renameSync,
  rmSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs';
const root = fileURLToPath(new URL('../../', import.meta.url));
const requested = process.argv.slice(2);
const backends = requested.length ? requested : ['webgl2', 'webgpu'];
if (backends.some((backend) => !['webgl2', 'webgpu'].includes(backend)))
  throw Error('Expected webgl2 or webgpu');
function run(command: string, args: string[]) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw Error(`${command} exited with ${result.status}`);
}
for (const backend of backends) {
  run('cargo', [
    'build',
    '--locked',
    '--manifest-path',
    'crates/simulation/Cargo.toml',
    '--no-default-features',
    '--features',
    backend,
    '--profile',
    'simulation-web',
    '--target',
    'wasm32-unknown-unknown',
  ]);
  const source = join(
    root,
    'crates/simulation/target/wasm32-unknown-unknown/simulation-web/fujimod_simulation.wasm',
  );
  const out = join(root, 'bindings/javascript/pkg/simulation', backend);
  const digest = (path: string) => createHash('sha256').update(readFileSync(path)).digest('hex');
  const stamp = JSON.stringify({
    backend,
    source: digest(source),
    script: digest(fileURLToPath(import.meta.url)),
  });
  const stampFile = join(out, 'build.json');
  try {
    if (
      readFileSync(stampFile, 'utf8') === stamp &&
      statSync(join(out, 'fujimod_simulation_bg.wasm')).size > 0 &&
      statSync(join(out, 'fujimod_simulation.js')).size > 0
    )
      continue;
  } catch {
    /* First build, or generated output was removed. */
  }
  mkdirSync(out, { recursive: true });
  const staging = mkdtempSync(join(out, '../.staging-'));
  try {
    run(process.env.WASM_BINDGEN ?? join(root, 'target/tooling/bin/wasm-bindgen'), [
      source,
      '--target',
      'web',
      '--out-dir',
      staging,
      '--out-name',
      'fujimod_simulation',
    ]);
    const wasm = join(staging, 'fujimod_simulation_bg.wasm');
    run(process.env.WASM_OPT ?? join(root, 'crates/simulation/tools/node_modules/.bin/wasm-opt'), [
      wasm,
      '-Oz',
      '--enable-bulk-memory',
      '--enable-nontrapping-float-to-int',
      '--enable-sign-ext',
      '--enable-reference-types',
      '--enable-multivalue',
      '--enable-mutable-globals',
      '-o',
      wasm,
    ]);
    const bytes = statSync(wasm).size;
    if (bytes > 64 * 1024 * 1024)
      throw Error(`Simulation Wasm exceeds its 64 MiB budget: ${bytes} bytes`);
    if (!WebAssembly.validate(readFileSync(wasm)))
      throw Error('Simulation Wasm requires experimental engine flags');
    // Publish only complete, optimized files; never stream an in-progress Wasm build.
    for (const file of readdirSync(staging)) renameSync(join(staging, file), join(out, file));
    writeFileSync(stampFile, stamp);
    console.log(`Optimized ${backend} simulation Wasm: ${bytes} bytes`);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}
