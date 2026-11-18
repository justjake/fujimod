import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'vitest';
import { cycloneDx, type Component } from '../scripts/sbom/bom.ts';
import { cargoComponents } from '../scripts/sbom/cargo.ts';
import { writeLicenseIndex } from '../scripts/sbom/licenses.ts';
import { emittedPackageDirectories, npmComponents } from '../scripts/sbom/npm.ts';
import { vendoredComponents } from '../scripts/sbom/vendor.ts';
import { linkedCrates } from '../../../bindings/javascript/scripts/linked-crates.ts';

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
  assert.equal(row.externalReferences[0].url, component.source);
  assert.deepEqual(
    row.properties.map(({ value }) => value),
    ['camera', 'emulator'],
  );
  assert.throws(
    () => cycloneDx([{ ...component, texts: [] }]),
    /missing SPDX license or license text/,
  );
});

test('Credits shortens commit versions and preserves full SBOM references and text', () => {
  const directory = mkdtempSync(join(tmpdir(), 'fujimod-license-index-'));
  try {
    mkdirSync(join(directory, 'server'));
    const version = '0'.repeat(39) + '1';
    const source = `https://example.org/fixture/tree/${version}`;
    const bom = cycloneDx([
      {
        name: 'fixture',
        version,
        source,
        license: 'MIT',
        texts: ['Full license\n'],
        products: ['website'],
      },
    ]);
    writeLicenseIndex(bom, directory);
    const [entry] = JSON.parse(
      readFileSync(join(directory, 'server/credits-components.json'), 'utf8'),
    );
    assert.equal(entry.version, version.slice(0, 12));
    assert.equal(bom.components[0].version, version);
    assert.equal(entry.source, source);
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
