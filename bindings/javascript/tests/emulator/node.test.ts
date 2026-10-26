import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { loadNodeModule } from '../../dist/emulator/node.mjs';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

test('Node loader resolves a module file and its adjacent Wasm binary', async () => {
  const target = resolve(packageRoot, '../../target');
  await mkdir(target, { recursive: true });
  const directory = await mkdtemp(join(target, 'javascript-emulator-node-'));
  try {
    const modulePath = join(directory, 'camera.mjs');
    const wasmPath = join(directory, 'camera.wasm');
    await writeFile(wasmPath, new Uint8Array([0, 97, 115, 109]));
    await writeFile(
      modulePath,
      `export default async ({ wasmBinary }) => {
        if (wasmBinary.byteLength !== 4) throw Error('adjacent binary was not loaded');
        return { configure: () => {}, Camera: class {} };
      };\n`,
    );
    const loaded = await loadNodeModule(modulePath);
    assert.equal(typeof loaded.configure, 'function');
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('browser entry remains independent of Node builtins', async () => {
  const entry = new URL('../../dist/emulator/index.js', import.meta.url);
  const source = await readFile(entry, 'utf8');
  assert.doesNotMatch(source, /from ['"]node:/);
  assert.doesNotMatch(source, /import\(['"]node:/);
});
