import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'vite-plus/test';
import { developmentAssets } from './assets';

test('development hydration URLs change with source and server revisions', async () => {
  const root = await mkdtemp(join(tmpdir(), 'fujimod-development-assets-'));
  try {
    await mkdir(join(root, 'src'));
    const file = join(root, 'src/Page.tsx');
    await writeFile(file, 'export default () => <main>Before</main>');
    const original = { js: ['/src/Page.tsx'], css: [{ id: 'style', content: 'main{color:red}' }] };
    const resolver = { resolve: () => original };
    const assets = developmentAssets(resolver, root);
    const first = await assets.resolve('src/Page.tsx');
    assert.ok(first);
    assert.match(first.js[0], /^\/src\/Page\.tsx\?development-revision=[a-f0-9]{64}$/);
    assert.deepEqual(await assets.resolve('src/Page.tsx'), first);
    assert.deepEqual(first.css, original.css);
    assert.deepEqual(original.js, ['/src/Page.tsx'], 'do not mutate the plugin manifest cache');
    await writeFile(file, 'export default () => <main>After</main>');
    const changed = await assets.resolve('src/Page.tsx');
    assert.notDeepEqual(changed?.js, first.js);
    assert.notDeepEqual(
      (await developmentAssets(resolver, root).resolve('src/Page.tsx'))?.js,
      changed?.js,
      'a server restart also invalidates compiler/configuration changes',
    );
    const absolute = developmentAssets(
      { resolve: () => ({ js: [`/@fs/${file}?import`], css: [] }) },
      root,
    );
    assert.match((await absolute.resolve('page'))!.js[0], /\?import=&development-revision=/);
    assert.equal(await developmentAssets({ resolve: () => null }, root).resolve('missing'), null);
    assert.throws(() => developmentAssets({}, root), /requires the Vite asset resolver/);
  } finally {
    await rm(root, { recursive: true });
  }
});
