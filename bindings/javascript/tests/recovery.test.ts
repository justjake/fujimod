import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, sdCardTools } from '../dist/index.js';

await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});

await test('recovery availability does not require an emulator board', () => {
  const status = sdCardTools.availability('xt30-201');
  try {
    assert.equal(status.firmwareId, 'xt30-201');
    assert.equal(status.generation, 'xp4');
    assert.equal(status.canBuild, true);
    assert.ok(status.outputFilenames.includes('ffinit.bin'));
    assert.ok(status.outputFilenames.includes('ffboot.bin'));
  } finally {
    status.free();
  }
});

await test('recovery exposes the documented power-on combinations', () => {
  const status = sdCardTools.availability('xs10-311');
  try {
    assert.deepEqual(status.sdBootCombination, {
      usb: false,
      hold: [{ id: 'quick_menu_button' }, { id: 'movie_recording_button' }],
    });
    assert.deepEqual(status.firmwareUpdate, { hold: [{ id: 'display_back_button' }] });
  } finally {
    status.free();
  }
});

await test('located XP5 startup recipes can build recovery cards', () => {
  for (const firmwareId of ['xm5-130', 'x100vi-131']) {
    const status = sdCardTools.availability(firmwareId);
    try {
      assert.equal(status.generation, 'xp5');
      assert.equal(status.canBuild, true);
      assert.deepEqual(status.missing, []);
      assert.ok(status.outputFilenames.includes('marble_init.bin'));
      assert.ok(status.outputFilenames.includes('ffboot.bin'));
    } finally {
      status.free();
    }
  }
});

await test('missing generation facts and incorrect DAT bytes are refused', () => {
  const status = sdCardTools.availability('unknown-update');
  try {
    assert.equal(status.canBuild, false);
    assert.ok(status.missing.length > 0);
    assert.equal(status.sdBootCombination, null);
    assert.equal(status.firmwareUpdate, null);
  } finally {
    status.free();
  }
  assert.throws(
    () => sdCardTools.build('xs10-311', new Uint8Array(4)),
    /Update size differs from recipe/,
  );
  assert.throws(() => sdCardTools.build('gfx100rf-113', new Uint8Array(4)), /Missing recovery facts/);
});

await test('green LED cards cover every processor generation without an updater DAT', () => {
  for (const firmwareId of [
    'x10-201',
    'xe1-271',
    'xt1-552',
    'gfx50r-311',
    'x100v-301',
    'x100vi-131',
  ]) {
    const status = sdCardTools.availability(firmwareId);
    try {
      assert.ok(status.greenLedFilenames.length > 0, firmwareId);
      assert.ok(status.greenLedFilenames.every((name) => name.endsWith('.bin')));
      assert.ok(!status.greenLedFilenames.includes('ffuser.bin'));
    } finally {
      status.free();
    }
    assert.throws(
      () => sdCardTools.buildGreenLed(firmwareId, new Uint8Array(4)),
      /Update size differs/,
    );
  }
  const unknown = sdCardTools.availability('unknown-update');
  try {
    assert.deepEqual(unknown.greenLedFilenames, []);
  } finally {
    unknown.free();
  }
});

await test('EXR II bodies that share a memory setup header share one card root', () => {
  const header = (firmwareId: string) => {
    const status = sdCardTools.availability(firmwareId);
    try {
      return status.sdBootV2Header;
    } finally {
      status.free();
    }
  };
  assert.match(header('x70-110') ?? '', /^[0-9a-f]{64}$/);
  assert.equal(header('xe2-411'), header('x70-110'));
  assert.equal(header('xe2s-111'), header('x70-110'));
  assert.notEqual(header('xt10-131'), header('x70-110'));
  assert.equal(header('x100v-301'), undefined);
  assert.throws(
    () => sdCardTools.buildSdBootV2SdCard('x70-110', new Uint8Array(4)),
    /Update size differs from recipe/,
  );
  assert.throws(() => sdCardTools.buildSdBootV2SdCard('x100v-301', new Uint8Array(4)), /requires EXR II/);
});
