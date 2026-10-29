import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stageSite, runtimeBundleFiles } from '../scripts/stage-site.ts';
import type { Catalog } from '../src/catalog';
import { cameraApi, validateRuntimeApi } from '../scripts/runtime-api';

test('runtime API validation distinguishes static methods and rejects missing boot release', () => {
  const declarations =
    'declare class Camera { static fromPrepared(): Camera; releaseBootInputs(): void; }';
  assert.throws(
    () => validateRuntimeApi('class Camera { static fromPrepared() {} }', declarations),
    /missing: releaseBootInputs/,
  );
  assert.throws(
    () =>
      validateRuntimeApi('class Camera { fromPrepared() {} releaseBootInputs() {} }', declarations),
    /missing: static fromPrepared/,
  );
  validateRuntimeApi(
    'function factory() { const Camera = class Camera { static fromPrepared() {} releaseBootInputs() {} }; }',
    declarations,
  );
});

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
    const inputs = { runtime, media: join(root, 'media') };
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
    await assert.rejects(stageSite(inputs, destination), /Runtime Camera API is missing/);
    const declarations = await readFile(
      new URL('../../../bindings/javascript/emulator/camera.generated.ts', import.meta.url),
      'utf8',
    );
    const loader = `class Camera { ${[...cameraApi(declarations)].map((method) => `${method}() {}`).join(' ')} }`;
    await writeFile(join(runtime, 'test-100/camera.mjs'), loader);
    await assert.rejects(stageSite(inputs, destination), /licenses/);
    await mkdir(join(runtime, 'test-100/licenses'));
    await writeFile(join(runtime, 'test-100/licenses/COPYING.txt'), 'license');
    catalog.firmwares[0].service_workflow = 'runtime/test-100/service-workflow.generated.json';
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify(catalog));
    await assert.rejects(stageSite(inputs, destination), /service-workflow.generated.json/);
    await writeFile(join(runtime, 'test-100/service-workflow.generated.json'), '{"schema":1}');
    await assert.rejects(stageSite(inputs, destination), /media.json/);
    catalog.firmwares.push({
      id: 'physical-100',
      body: 'physical',
      model: 'Physical',
      version: '1.00',
      sha256: '3'.repeat(64),
      bytes: 42,
      release_date: '',
      download_url: 'https://example.org/physical',
      service_workflow: 'runtime/physical-100/service-workflow.generated.json',
    });
    await mkdir(join(runtime, 'physical-100'));
    await writeFile(
      join(runtime, 'physical-100/service-workflow.generated.json'),
      '{"schema":1,"physical":true}',
    );
    await writeFile(join(runtime, 'catalog.json'), JSON.stringify(catalog));
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
    await assert.rejects(stageSite(inputs, destination), /media.json/);
    assert.equal(await readFile(join(destination, 'runtime/catalog.json'), 'utf8'), 'installed');
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
    await writeFile(join(runtime, 'test-100-usb/camera.mjs'), loader);
    await writeFile(join(runtime, 'test-100-usb/wasm.json'), '{"construction":"session"}');
    await stageSite(inputs, destination);
    for (const [name, expected] of [
      ['runtime/test-100/service-workflow.generated.json', '{"schema":1}'],
      ['runtime/physical-100/service-workflow.generated.json', '{"schema":1,"physical":true}'],
      ['runtime/test-100/licenses/COPYING.txt', 'license'],
      ['runtime/test-100-usb/licenses/COPYING.txt', 'usb licenses/COPYING.txt'],
    ])
      assert.equal(await readFile(join(destination, name), 'utf8'), expected);
    await assert.rejects(readFile(join(destination, 'artifacts/patches.json')), /ENOENT/);
    const manifest = JSON.parse(await readFile(join(destination, 'site-assets.json'), 'utf8')) as {
      files: { path: string }[];
    };
    assert.ok(
      manifest.files.some(
        (file) => file.path === 'runtime/test-100/service-workflow.generated.json',
      ),
    );
    assert.ok(
      manifest.files.some((file) => file.path === 'runtime/test-100-usb/licenses/COPYING.txt'),
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('prepared runtime staging follows declared cold input filenames and refuses traversal', () => {
  assert.deepEqual(
    runtimeBundleFiles({ construction: 'resolved', dmpCpuRenderer: false }),
    [
      'camera.wasm',
      'camera.mjs',
      'wasm.json',
      'runtime-NOTICES.txt',
      'dmp_webgpu.js',
      'dmp_webgpu_bg.wasm',
    ],
  );
  assert.throws(
    () => runtimeBundleFiles({ construction: 'resolved', dmpCpuRenderer: 'webgpu' }),
    /invalid DMP renderer selection/,
  );
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
