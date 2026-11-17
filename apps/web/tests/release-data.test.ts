import { mkdtemp, mkdir, rm, utimes, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, vi } from 'vite-plus/test';
import { loadReleaseData } from '../scripts/release-data';

test('reads every page and uses publication dates, including published prereleases', async () => {
  const newest = {
    tag_name: 'v2.0.0',
    published_at: '2026-04-02T00:05:00Z',
    body: '## New features',
    draft: false,
    prerelease: true,
  };
  const oldest = {
    tag_name: 'v1.0.0',
    published_at: '2026-03-31T23:55:00Z',
    body: null,
    draft: false,
  };
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(
      Response.json([oldest, { ...newest, tag_name: 'draft', draft: true }], {
        headers: {
          Link: '<https://api.github.com/repos/justjake/fujimod/releases?page=2>; rel="next"',
        },
      }),
    )
    .mockResolvedValueOnce(Response.json([newest]));
  const data = await loadReleaseData({ preview: false }, request);
  expect(request.mock.calls.map(([url]) => url)).toEqual([
    'https://api.github.com/repos/justjake/fujimod/releases?per_page=100',
    'https://api.github.com/repos/justjake/fujimod/releases?page=2',
  ]);
  expect(data.current).toEqual({
    version: 'v2.0.0',
    publishedAt: '2026-04-02T00:05:00.000Z',
    notes: '## New features',
  });
  expect(data.past).toEqual([
    { version: 'v1.0.0', publishedAt: '2026-03-31T23:55:00.000Z', notes: '' },
  ]);
  expect(data.personalNote.daysSinceFirstRelease).toBe(2);
});

test('chooses the newest local version and reads the contributor roster offline', async () => {
  const root = await mkdtemp(join(tmpdir(), 'site-release-data-'));
  try {
    const notesDirectory = join(root, 'release-notes');
    const contributorsDirectory = join(root, 'contributors');
    await Promise.all([mkdir(notesDirectory), mkdir(contributorsDirectory)]);
    const old = join(notesDirectory, '0.9.0.md');
    const current = join(notesDirectory, '0.10.0.md');
    await Promise.all([
      writeFile(old, '# Earlier'),
      writeFile(current, '# Latest'),
      writeFile(join(notesDirectory, 'README.md'), 'Ignored'),
      writeFile(join(contributorsDirectory, 'z.json'), '{"attributionName":"@zoe"}'),
      writeFile(join(contributorsDirectory, 'a.json'), '{"attributionName":"Ada Example"}'),
      writeFile(join(contributorsDirectory, 'README.md'), 'Ignored'),
    ]);
    await Promise.all([
      utimes(old, new Date('2026-03-01'), new Date('2026-03-01')),
      utimes(current, new Date('2026-03-07'), new Date('2026-03-07')),
    ]);
    const request = vi.fn<typeof fetch>().mockRejectedValue(new Error('Offline'));
    const data = await loadReleaseData(
      { preview: true, notesDirectory, contributorsDirectory },
      request,
    );
    expect(request).not.toHaveBeenCalled();
    expect(data.current).toEqual({
      version: '0.10.0',
      publishedAt: '2026-03-07T00:00:00.000Z',
      notes: '# Latest',
    });
    expect(data.past.map(({ version }) => version)).toEqual(['0.9.0']);
    expect(data.personalNote).toEqual({
      daysSinceFirstRelease: 6,
      contributorCount: 2,
      contributorNames: ['Ada Example', '@zoe'],
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('fails a production build when GitHub cannot supply release data', async () => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 503 }));
  await expect(loadReleaseData({ preview: false }, request)).rejects.toThrow('HTTP 503');
});
