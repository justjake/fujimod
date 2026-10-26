import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, recovery } from '../dist/index.js';

await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});

test('Agate recovery is available without a complete emulator board', () => {
  const status = recovery.availability('xpro2-511');
  try {
    assert.equal(status.generation, 'xpp');
    assert.equal(status.canBuild, true);
    assert.deepEqual(status.missing, []);
    assert.deepEqual(status.outputFilenames, ['ffsys.bin', 'ffboot.bin', 'FWUP0008.DAT']);
  } finally {
    status.free();
  }
  assert.throws(() => recovery.build('xpro2-511', new Uint8Array(4)), /size differs/);
});

test('EXR exposes its third file and the unresolved updater contract', () => {
  for (const [id, generation] of [['xf1-101', 'exr'], ['xpro1-382', 'exrpro']] as const) {
    const status = recovery.availability(id);
    try {
      assert.equal(status.generation, generation);
      assert.equal(status.canBuild, false);
      assert.deepEqual(status.outputFilenames, ['ffsys.bin', 'ffboot.bin', 'ffimage.bin', 'FPUPDATE.DAT']);
      assert.deepEqual(status.missing, ['version-independent stock updater acceptance']);
    } finally {
      status.free();
    }
    assert.throws(() => recovery.build(id, new Uint8Array(4)), /stock updater acceptance/);
  }
});
