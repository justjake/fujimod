import { test, expect } from 'vite-plus/test';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { localReleaseMiddleware } from '../scripts/local-releases.ts';
import { loadRelease } from '../src/patch/release.ts';

test('development routes serve the configured release, observe edits, and refuse other files/tags', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'local-release-'));
  const options = { artifacts: directory, notes: undefined as string | undefined };
  const middleware = localReleaseMiddleware(options);
  const server = createServer((request, response) =>
    middleware(request, response, () => {
      response.statusCode = 404;
      response.end();
    }),
  );
  try {
    const digest = 'a'.repeat(64),
      index = {
        schema: 2,
        release: 'v2',
        built_utc: '2026-10-26T00:00:00Z',
        patches: { [digest]: 'test.fmpatch' },
        notes: { name: 'notes.md' },
        firmwares: [
          {
            id: 'test-100',
            model: 'Test',
            version: '1.00',
            sha256: digest,
            bytes: 1,
            release_date: '2026-01-01',
            download_url: 'https://example.org/',
            patch_sha256: digest,
            patch_bytes: 4,
            boot_instructions: ['Copy files.'],
          },
        ],
      };
    await writeFile(join(directory, 'patches.json'), JSON.stringify(index));
    await writeFile(join(directory, 'test.fmpatch'), 'data');
    await writeFile(join(directory, 'notes.md'), 'note');
    await writeFile(join(directory, 'secret.txt'), 'secret');
    await mkdir(join(directory, 'nested'));
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (!address || typeof address === 'string') throw Error('Missing test server address');
    const base = `http://127.0.0.1:${address.port}`;
    const current = await fetch(`${base}/releases/current`);
    expect(current.headers.get('cache-control')).toBe('no-store');
    expect(await current.json()).toEqual({
      tag: 'v2',
      name: 'v2',
      published_at: index.built_utc,
      prerelease: true,
    });
    for (const file of ['patches.json', 'test.fmpatch', 'notes.md'])
      expect((await fetch(`${base}/releases/v2/${file}`)).status).toBe(200);
    await writeFile(join(directory, 'notes.md'), 'edited');
    expect(await (await fetch(`${base}/releases/v2/notes.md`)).text()).toBe('edited');
    const head = await fetch(`${base}/releases/v2/notes.md`, { method: 'HEAD' });
    expect(head.headers.get('content-length')).toBe('6');
    expect(await head.text()).toBe('');
    for (const path of [
      '/releases/old/notes.md',
      '/releases/v2/secret.txt',
      '/releases/v2/%2fsecret',
      '/releases/v2/bad%zz',
    ])
      expect((await fetch(base + path)).status).toBe(404);
    expect((await fetch(`${base}/releases/current`, { method: 'POST' })).status).toBe(405);

    // Keep the artifact copy stale: editing the source must still reach the real loader.
    options.notes = join(directory, 'editable-notes');
    await mkdir(options.notes);
    const source = join(options.notes, 'v2.md');
    await writeFile(source, '- **Feature:** Regular description.');
    expect((await loadRelease(base))?.notesText).toBe('- **Feature:** Regular description.');
    await writeFile(source, '- **Feature:** Revised description.');
    expect((await loadRelease(base))?.notesText).toBe('- **Feature:** Revised description.');
    expect(await (await fetch(`${base}/releases/v2/patches.json`)).json()).toEqual(index);
    expect(await (await fetch(`${base}/releases/v2/test.fmpatch`)).text()).toBe('data');
    const sourceHead = await fetch(`${base}/releases/v2/notes.md`, { method: 'HEAD' });
    expect(sourceHead.headers.get('content-length')).toBe('35');
    expect(await sourceHead.text()).toBe('');

    // A missing configured source refuses the request instead of showing the old copy.
    await rm(source);
    expect((await fetch(`${base}/releases/v2/notes.md`)).status).toBe(404);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(directory, { recursive: true, force: true });
  }
});
