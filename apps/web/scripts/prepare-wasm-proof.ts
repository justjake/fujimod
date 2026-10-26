import { cp, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { wasmFixture } from '../tests/wasm-fixture.ts';
import { wasmSessionFixture } from '../tests/wasm-session-fixture.ts';
const [source, destination = process.env.FUJIMOD_RUNTIME ?? 'public/runtime'] = process.argv.slice(2);
if (!source) throw Error('Usage: node scripts/prepare-wasm-proof.ts <module-directory> [runtime-directory]');
const runtime = resolve(destination);
for (const fault of [false, true]) {
  const output = resolve(runtime, fault ? 'wasm-proof-fault' : 'wasm-proof');
  await mkdir(output, { recursive: true });
  const fixture = await wasmFixture(fault);
  fixture.manifest.inputs = fixture.inputs.inputs;
  if (fault) {
    fixture.manifest.module = '../wasm-proof/camera.mjs';
    fixture.manifest.wasm = '../wasm-proof/camera.wasm';
  } else {
    for (const name of ['camera.mjs', 'camera.wasm'])
      await cp(resolve(source, name), resolve(output, name));
  }
  await writeFile(resolve(output, 'program.dat'), fixture.dat);
  await writeFile(resolve(output, 'wasm.json'), JSON.stringify(fixture.manifest));
}

const output = resolve(runtime, 'wasm-session-proof');
await mkdir(output, { recursive: true });
const fixture = await wasmSessionFixture();
await writeFile(resolve(output, 'program.dat'), fixture.dat);
await writeFile(resolve(output, 'wasm.json'), JSON.stringify(fixture.manifest));
