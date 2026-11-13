import { afterEach, expect, test, vi } from 'vite-plus/test';
import { downloadSession, reportDownload } from './report.ts';

afterEach(() => vi.unstubAllGlobals());

function sessionStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  };
}

test('a download beacon carries one tab-scoped session', async () => {
  const storage = sessionStorage();
  const beacons: { url: string; blob: Blob }[] = [];
  vi.stubGlobal('window', {});
  vi.stubGlobal('sessionStorage', storage);
  vi.stubGlobal('navigator', {
    sendBeacon: (url: string, blob: Blob) => {
      beacons.push({ url, blob });
      return true;
    },
  });

  reportDownload({ release: '0.0.1', kind: 'installable', firmware: 'a'.repeat(64), cameras: 1 });
  reportDownload({ release: '0.0.1', kind: 'sd-card', cameras: 2 });

  expect(beacons).toHaveLength(2);
  expect(beacons[0]?.url).toBe('/api/downloads');
  const first = JSON.parse(await beacons[0]!.blob.text()) as Record<string, unknown>;
  const second = JSON.parse(await beacons[1]!.blob.text()) as Record<string, unknown>;
  expect(first).toMatchObject({ release: '0.0.1', kind: 'installable', cameras: 1 });
  expect(second).toMatchObject({ release: '0.0.1', kind: 'sd-card', cameras: 2 });
  expect(first.session).toBe(second.session);
  expect(first.session).toBe(downloadSession());
});

test('a refused beacon falls back to keepalive fetch', async () => {
  vi.stubGlobal('window', {});
  vi.stubGlobal('sessionStorage', sessionStorage());
  vi.stubGlobal('navigator', { sendBeacon: () => false });
  const fetcher = vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(null, { status: 204 }),
  );
  vi.stubGlobal('fetch', fetcher);

  reportDownload({ release: '0.0.1', kind: 'sd-card', cameras: 3 });

  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]?.[0]).toBe('/api/downloads');
  expect(fetcher.mock.calls[0]?.[1]).toMatchObject({ method: 'POST', keepalive: true });
});

test('an invalid saved session is replaced instead of sent', () => {
  const storage = sessionStorage();
  storage.setItem('fujimod.download-session', 'not-a-uuid');
  vi.stubGlobal('sessionStorage', storage);

  const created = downloadSession();

  expect(created).not.toBe('not-a-uuid');
  expect(storage.getItem('fujimod.download-session')).toBe(created);
});
