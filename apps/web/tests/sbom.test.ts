import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'vitest';
import { cycloneDx, type Component } from '../scripts/sbom/bom.ts';
import { cargoComponents } from '../scripts/sbom/cargo.ts';
import { npmComponents } from '../scripts/sbom/npm.ts';
import { vendoredComponents } from '../scripts/sbom/vendor.ts';

test('the lockfile contributes production dependencies and excludes development tools', () => {
  const root = resolve(import.meta.dirname, '../../..');
  const components = npmComponents(root, []);
  assert.ok(components.some(({ name, texts }) => name === 'solid-js' && texts[0].includes('MIT')));
  assert.ok(components.some(({ name }) => name === 'Lucide'));
  assert.ok(components.every(({ products }) => products.join() === 'website'));
  assert.ok(!components.some(({ name }) => ['typescript', 'vitest', 'drizzle-kit'].includes(name)));
});

test('the WebUSB Wasm graph excludes host-side Rust macros and build tools', () => {
  const root = resolve(import.meta.dirname, '../../..');
  const components = cargoComponents(root, [
    { package: 'fujimod-javascript', target: 'wasm32-unknown-unknown', product: 'website' },
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
});

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
