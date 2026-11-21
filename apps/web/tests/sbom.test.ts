import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'vite-plus/test';
import { cycloneDx, type Component } from '../scripts/sbom/bom.ts';
import { cargoComponents } from '../scripts/sbom/cargo.ts';
import { writeLicenseIndex } from '../scripts/sbom/licenses.ts';
import { emittedPackageDirectories, npmComponents } from '../scripts/sbom/npm.ts';
import { vendoredComponents } from '../scripts/sbom/vendor.ts';
import { linkedCrates } from '../../../bindings/javascript/scripts/linked-crates.ts';
import { readRuntimeSbom, runtimeSbom, type EmulatorLinks } from '../scripts/sbom/runtime.ts';

test('emitted npm inputs select packages and the lockfile supplies their versions', () => {
  const root = resolve(import.meta.dirname, '../../..');
  const paths = emittedPackageDirectories(
    ['node_modules/solid-js/package.json'],
    join(root, 'apps/web'),
  );
  const components = npmComponents(root, paths);
  assert.ok(components.some(({ name, texts }) => name === 'solid-js' && texts[0].includes('MIT')));
  assert.ok(components.some(({ name }) => name === 'Lucide'));
  assert.ok(components.every(({ products }) => products.join() === 'website'));
  assert.deepEqual(
    components.map(({ name }) => name),
    ['solid-js', 'Lucide'],
  );
  const [solid] = components;
  assert.deepEqual(cycloneDx([solid]).components[0].externalReferences, [
    { type: 'distribution', url: solid.source },
    { type: 'website', url: `https://www.npmjs.com/package/solid-js/v/${solid.version}` },
  ]);
});

test('only nonempty linked Rust objects count, with exact artifact versions', () => {
  const artifacts = new Map([
    ['first-aaaa', '/crates/first-1.0.0'],
    ['first-bbbb', '/crates/first-2.0.0'],
    ['second-cccc', '/crates/second-1.0.0'],
    ['unused-dddd', '/crates/unused-1.0.0'],
  ]);
  const map = [
    '       -    8c779      954         /build/emulator.a(first-aaaa.first.cgu.0.rcgu.o):(run)',
    '       -    8d0cd        0         /build/emulator.a(first-bbbb.first.cgu.0.rcgu.o):(discarded)',
    '       -    8d0cd       2d         /build/libsecond-cccc.rlib(second.cccc-cgu.0.rcgu.o):(next)',
  ].join('\n');
  assert.deepEqual(linkedCrates(map, artifacts), ['/crates/first-1.0.0', '/crates/second-1.0.0']);
});

test('runtime SBOM requires linker evidence for the exact staged Wasm', () => {
  const runtime = mkdtempSync(join(tmpdir(), 'fujimod-runtime-sbom-'));
  try {
    const bundle = join(runtime, 'camera');
    mkdirSync(bundle);
    writeFileSync(
      join(runtime, 'catalog.json'),
      JSON.stringify({ firmwares: [{ emulate: { bundle: 'runtime/camera/' } }] }),
    );
    writeFileSync(join(bundle, 'wasm.json'), '{}');
    writeFileSync(join(bundle, 'camera.wasm'), 'abc');
    const receipt = join(runtime, 'sbom-components.json');
    writeFileSync(receipt, JSON.stringify({ schema: 2, components: [] }));
    assert.throws(() => readRuntimeSbom(runtime, receipt), /no Wasm digests/);

    const sha256 = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
    const bom = { schema: 3, wasm: { 'camera/camera.wasm': sha256 }, components: [] };
    writeFileSync(receipt, JSON.stringify(bom));
    assert.deepEqual(readRuntimeSbom(runtime, receipt), bom);

    const module = { sha256, linkedDirectories: [] };
    const links: EmulatorLinks = {
      perf: false,
      rust: '',
      emscripten: '',
      modules: { 'camera.wasm': module, 'dmp_webgpu_bg.wasm': module },
    };
    writeFileSync(join(bundle, 'camera.wasm'), 'changed');
    assert.throws(() => readRuntimeSbom(runtime, receipt), /does not match the staged Wasm/);
    assert.throws(() => runtimeSbom(runtime, runtime, [], links), /No matching linker evidence/);
  } finally {
    rmSync(runtime, { recursive: true, force: true });
  }
});

test('the WebUSB Wasm graph excludes host-side Rust macros and build tools', () => {
  const root = resolve(import.meta.dirname, '../../..');
  const linkedDirectories: string[] = JSON.parse(
    readFileSync(join(root, 'bindings/javascript/.tools/webusb/linked-crates.json'), 'utf8'),
  );
  const components = cargoComponents(root, [
    {
      package: 'fujimod-javascript',
      target: 'wasm32-unknown-unknown',
      product: 'website',
      linkedDirectories,
    },
  ]);
  assert.ok(
    components.some(
      ({ name, source }) =>
        name === 'wasm-bindgen' && source.startsWith('https://static.crates.io/'),
    ),
  );
  assert.ok(
    !components.some(({ name }) => ['wasm-bindgen-macro', 'fujimod-bindgen'].includes(name)),
  );
}, 30_000);

test('CycloneDX retains the declared expression, full texts, source, and every shipping product', () => {
  const component: Component = {
    name: 'fixture',
    version: '1.2.3',
    purl: 'pkg:cargo/fixture@1.2.3',
    source: 'https://example.org/fixture-1.2.3.crate',
    license: 'MIT OR Apache-2.0',
    texts: ['First license\n', 'Second license\n'],
    products: ['emulator'],
  };
  const bom = cycloneDx([component, { ...component, products: ['camera'] }]);
  assert.equal(bom.specVersion, '1.6');
  assert.equal(bom.components.length, 1);
  const [row] = bom.components;
  assert.deepEqual(row.licenses, [
    { expression: 'MIT OR Apache-2.0', acknowledgement: 'declared' },
  ]);
  assert.equal(row.evidence?.licenses[0].license.text.content, component.texts.join('\n\n'));
  assert.deepEqual(row.externalReferences, [
    { type: 'distribution', url: component.source },
    { type: 'website', url: 'https://crates.io/crates/fixture/1.2.3' },
  ]);
  assert.deepEqual(
    row.properties.map(({ value }) => value),
    ['camera', 'emulator'],
  );
  assert.throws(
    () => cycloneDx([{ ...component, texts: [] }]),
    /missing SPDX license or license text/,
  );
  assert.throws(
    () => cycloneDx([{ ...component, purl: undefined }]),
    /missing upstream package page/,
  );
});

test('Credits shortens commit versions and preserves full SBOM references and text', () => {
  const directory = mkdtempSync(join(tmpdir(), 'fujimod-license-index-'));
  try {
    mkdirSync(join(directory, 'server'));
    const version = '0'.repeat(39) + '1';
    const source = `https://example.org/fixture/tree/${version}`;
    const packageSource = 'https://registry.npmjs.org/@scope/package/-/package-1.2.3.tgz';
    const fatFsArchive = 'https://elm-chan.org/fsw/ff/arc/ff16.zip';
    const bom = cycloneDx([
      {
        name: 'fixture',
        version,
        source,
        license: 'MIT',
        texts: ['Full license\n'],
        products: ['website'],
      },
      {
        name: '@scope/package',
        version: '1.2.3',
        source: packageSource,
        purl: 'pkg:npm/%40scope/package@1.2.3',
        license: 'MIT',
        texts: ['Full license\n'],
        products: ['website'],
      },
      {
        name: 'FatFs',
        version: 'R0.16',
        source: fatFsArchive,
        license: 'BSD-1-Clause',
        texts: ['Full license\n'],
        products: ['camera'],
      },
    ]);
    writeLicenseIndex(bom, directory);
    const [npm, fatFs, entry] = JSON.parse(
      readFileSync(join(directory, 'server/credits-components.json'), 'utf8'),
    );
    assert.equal(entry.version, version.slice(0, 12));
    assert.equal(bom.components.find(({ name }) => name === 'fixture')?.version, version);
    assert.equal(entry.source, source);
    assert.equal(npm.source, 'https://www.npmjs.com/package/@scope/package/v/1.2.3');
    assert.equal(
      fatFs.source,
      'https://github.com/zephyrproject-rtos/fatfs/tree/18ad3932f7f18b59abf99cb2f8a6cd5d6746cfe0',
    );
    assert.equal(
      readFileSync(join(directory, 'client', entry.licensePath), 'utf8'),
      'Full license\n',
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('a shipped vendored source without local licensing fails closed', () => {
  const root = mkdtempSync(join(tmpdir(), 'fujimod-sbom-'));
  try {
    mkdirSync(join(root, 'vendor'));
    assert.throws(() => vendoredComponents(root, root, 'camera'), /missing upstream.json/);
    writeFileSync(
      join(root, 'upstream.json'),
      JSON.stringify([
        {
          name: 'fixture',
          license: 'MIT',
          version: '1.2.3',
          source: 'https://example.org/fixture/1.2.3',
          files: ['vendor/LICENSE'],
        },
      ]),
    );
    assert.throws(() => vendoredComponents(root, root, 'camera'), /vendor.*LICENSE/);
    writeFileSync(join(root, 'vendor/LICENSE'), 'Full license text\n');
    assert.deepEqual(vendoredComponents(root, root, 'camera')[0].texts, ['Full license text\n']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
