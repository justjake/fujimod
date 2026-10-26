import { createHash } from 'node:crypto';
import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stageSite, runtimeBundleFiles } from '../scripts/stage-site.ts';
import type { Catalog } from '../src/catalog';

test('local preparation gates qualification and stages complete runtime bundles', async () => {
  const root = await mkdtemp(join(tmpdir(), 'site-assets-'));
  const runtime = join(root, 'input'),
    destination = join(root, 'public');
  const catalog: Catalog = {
    schema: 1,
    firmwares: [
      {
        id: 'test-100',
        body: 'test',
        model: 'Test',
        version: '1.00',
        sha256: '1'.repeat(64),
        bytes: 42,
        release_date: '',
        download_url: 'https://example.org/firmware',
        emulate: {
          bundle: 'runtime/test-100/',
          sceneInput: false,
        },
      },
    ],
  };
  try {
    await mkdir(join(destination, 'runtime'), { recursive: true });
    await writeFile(join(destination, 'runtime/catalog.json'), 'installed');
    await mkdir(join(runtime, 'test-100'), { recursive: true });
    const inputs = { runtime, media: join(root, 'media'), patches: join(root, 'patches') };
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify({ schema: 1, firmwares: [] }));
    await assert.rejects(stageSite(inputs, destination), /requires runnable emulators/);
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify(catalog));
    await assert.rejects(stageSite(inputs, destination), /qualification.json/);
    await writeFile(
      join(runtime, 'test-100/qualification.json'),
      JSON.stringify({
        firmware_sha256: '1'.repeat(64),
        execution_mode: 'strict',
        counts_as_boot: true,
        menu: true,
        failure: null,
      }),
    );
    await writeFile(
      join(runtime, 'test-100/browser-qualification.json'),
      JSON.stringify({ sha256: '2'.repeat(64), passed: true }),
    );
    await assert.rejects(stageSite(inputs, destination), /qualification failed/);
    await writeFile(
      join(runtime, 'test-100/browser-qualification.json'),
      JSON.stringify({ sha256: '1'.repeat(64), passed: true }),
    );
    await writeFile(
      join(runtime, 'test-100/wasm.json'),
      JSON.stringify({ construction: 'session' }),
    );
    await assert.rejects(stageSite(inputs, destination), /camera.wasm/);
    for (const name of ['camera.wasm', 'camera.mjs', 'wasm.json', 'runtime-NOTICES.txt'])
      if (name !== 'wasm.json') await writeFile(join(runtime, 'test-100', name), 'test');
    await assert.rejects(stageSite(inputs, destination), /licenses/);
    await mkdir(join(runtime, 'test-100/licenses'));
    await writeFile(join(runtime, 'test-100/licenses/COPYING.txt'), 'license');
    catalog.firmwares[0].camera_table = 'runtime/test-100/camera.json';
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify(catalog));
    await assert.rejects(stageSite(inputs, destination), /camera.json/);
    await writeFile(join(runtime, 'test-100/camera.json'), '{"schema":1}');
    await assert.rejects(stageSite(inputs, destination), /patches.json/);
    const prepared = {
      document: 'document.json',
      bootRom: 'boot-rom.bin',
      nand: 'browser-nand.bin.gz',
      settings: 'mcu-settings.bin',
    };
    await writeFile(
      join(runtime, 'test-100/wasm.json'),
      JSON.stringify({ construction: 'prepared', prepared }),
    );
    await assert.rejects(stageSite(inputs, destination), /document.json/);
    for (const name of Object.values(prepared))
      await writeFile(join(runtime, 'test-100', name), 'input');
    await assert.rejects(stageSite(inputs, destination), /patches.json/);
    assert.equal(await readFile(join(destination, 'runtime/catalog.json'), 'utf8'), 'installed');
    const hash = (data: Uint8Array) => createHash('sha256').update(data).digest('hex');
    const program = new Uint8Array(20);
    const metadata = Buffer.from(
      JSON.stringify({
        schema: 1,
        source: { bytes: 42, sha256: '1'.repeat(64) },
        outputs: {
          ffboot: { bytes: 1, sha256: '2'.repeat(64) },
          ffsys: { bytes: 1, sha256: '3'.repeat(64) },
        },
        commands: 1,
        program_sha256: hash(program),
        firmware: 'test-100',
        release: 'test',
        mods: [],
      }),
    );
    const header = Buffer.alloc(12);
    header.write('FJMODP01');
    header.writeUInt32LE(metadata.length, 8);
    const patch = Buffer.concat([header, metadata, program]);
    await mkdir(inputs.patches);
    await writeFile(join(inputs.patches, 'test.fmpatch'), patch);
    await writeFile(
      join(inputs.patches, 'patches.json'),
      JSON.stringify({
        schema: 1,
        release: 'test',
        patches: { ['1'.repeat(64)]: 'test.fmpatch' },
        firmwares: [
          {
            ...catalog.firmwares[0],
            release_date: '2026-01-01',
            patch_bytes: patch.length,
            patch_sha256: hash(patch),
            boot_instructions: ['Copy files.'],
          },
        ],
      }),
    );
    const video = 'a'.repeat(24) + '.mp4',
      still = 'a'.repeat(24) + '.webp';
    await mkdir(inputs.media);
    await writeFile(join(inputs.media, video), 'video');
    await writeFile(join(inputs.media, still), 'still');
    await writeFile(
      join(inputs.media, 'media.json'),
      JSON.stringify({
        schema: 1,
        cards: { shape_aspect_item: { video, still, clips: {} } },
      }),
    );
    catalog.firmwares[0].emulate!.usb = { bundle: 'runtime/test-100-usb/' };
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify(catalog));
    await mkdir(join(runtime, 'test-100-usb/licenses'), { recursive: true });
    for (const name of ['camera.wasm', 'camera.mjs', 'runtime-NOTICES.txt', 'licenses/COPYING.txt'])
      await writeFile(join(runtime, 'test-100-usb', name), 'usb ' + name);
    await writeFile(join(runtime, 'test-100-usb/wasm.json'), '{"construction":"session"}');
    await stageSite(inputs, destination);
    for (const [name, expected] of [
      ['runtime/test-100/camera.json', '{"schema":1}'],
      ['runtime/test-100/licenses/COPYING.txt', 'license'],
      ['runtime/test-100-usb/licenses/COPYING.txt', 'usb licenses/COPYING.txt'],
    ])
      assert.equal(await readFile(join(destination, name), 'utf8'), expected);
    const manifest = JSON.parse(await readFile(join(destination, 'site-assets.json'), 'utf8')) as {
      files: { path: string }[];
    };
    assert.ok(manifest.files.some((file) => file.path === 'runtime/test-100/camera.json'));
    assert.ok(
      manifest.files.some((file) => file.path === 'runtime/test-100-usb/licenses/COPYING.txt'),
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('prepared runtime staging follows declared cold input filenames and refuses traversal', () => {
  const prepared = {
    document: 'document.json',
    bootRom: 'boot-rom.bin',
    nand: 'nand.gz',
    settings: 'mcu.bin',
  };
  assert.deepEqual(
    runtimeBundleFiles({ construction: 'prepared', prepared }).slice(4),
    Object.values(prepared),
  );
  assert.throws(
    () =>
      runtimeBundleFiles({
        construction: 'prepared',
        prepared: { ...prepared, nand: '../nand.gz' },
      }),
    /invalid nand filename/,
  );
  assert.deepEqual(
    runtimeBundleFiles({
      construction: 'prepared',
      prepared: { ...prepared, camera: { configuration: 'nand-configuration.bin.zst' } },
    }).slice(8),
    ['nand-configuration.bin.zst'],
  );
  assert.throws(
    () =>
      runtimeBundleFiles({
        construction: 'prepared',
        prepared: { ...prepared, camera: { configuration: '../bank.zst' } },
      }),
    /invalid camera configuration filename/,
  );
  assert.throws(() => runtimeBundleFiles({ construction: 'prepared' }), /no bundle files/);
  assert.deepEqual(
    runtimeBundleFiles({
      construction: 'prepared',
      prepared: {
        document: 'document.json',
        bootRom: 'boot.bin',
      },
    }).slice(4),
    ['document.json', 'boot.bin'],
  );
});
