import { beforeEach, expect, test, vi } from 'vite-plus/test';

const v2 = { version: '0.2.1', publishedAt: '2026-11-22T22:37:59.000Z', notes: '## New' };
const v1 = { version: '0.2.0', publishedAt: '2026-11-21T00:07:23.000Z', notes: '## Old' };

beforeEach(() => vi.resetModules());

test('the Worker list replaces the build-time releases, newest first', async () => {
  const { loadLiveReleases, releaseData } = await import('../src/release-data');
  const built = releaseData();
  const fetcher = vi.fn<typeof fetch>(async () => Response.json([v1, v2]));
  await loadLiveReleases(fetcher);
  await loadLiveReleases(fetcher);
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0][0]).toBe('/releases/list');
  expect(releaseData().current).toEqual(v2);
  expect(releaseData().past).toEqual([v1]);
  expect(releaseData().personalNote).toEqual(built.personalNote);
});

test('a refused, malformed or empty list keeps the build-time releases', async () => {
  for (const response of [
    () => new Response(null, { status: 502 }),
    () => Response.json({ releases: [] }),
    () => Response.json([{ version: '0.2.1' }]),
    () => Response.json([]),
    () => Promise.reject(new TypeError('offline')),
  ]) {
    vi.resetModules();
    const { loadLiveReleases, releaseData } = await import('../src/release-data');
    const built = releaseData();
    await loadLiveReleases(vi.fn<typeof fetch>(async () => response()));
    expect(releaseData()).toBe(built);
  }
});
