import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import worker, { serveOta, type ReleaseCache, type ReleaseStore } from '../worker.ts';

const firmware = 'x70-110';
const release = {
  tag: 'v2',
  name: 'Release v2',
  published_at: '2026-11-01T00:00:00Z',
  prerelease: false,
};
const manifest = {
  firmware,
  version: '1.2.3',
  fs: {
    '/etc/ssl/cert.pem': {
      sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      size: 0,
    },
  },
};
const storeName = 'BA7816BF.8F0';
const manifestUrl = `https://fujimod.com/ota/${firmware}/latest.json`;
const releaseUrl = 'https://fujimod.com/ota/release.json';
const storeUrl = `https://fujimod.com/ota/store/${storeName}`;
const manifestAsset = `${firmware}-latest.json`;
const fetchedAt = 'x-fujimod-fetched-at';

afterEach(() => {
  vi.restoreAllMocks();
});

type Handler = (url: string, init?: RequestInit) => Response | undefined;

function releaseList() {
  return [
    {
      tag_name: release.tag,
      name: release.name,
      published_at: release.published_at,
      draft: false,
      prerelease: false,
      assets: [{ name: 'patches.json', state: 'uploaded' }],
    },
  ];
}

function api(handler?: Handler): Handler {
  return (url, init) =>
    url.startsWith('https://api.github.com')
      ? (handler?.(url, init) ?? Response.json(releaseList()))
      : undefined;
}

function combine(...handlers: Handler[]): Handler {
  return (url, init) => {
    for (const handler of handlers) {
      const response = handler(url, init);
      if (response) return response;
    }
  };
}

function download(suffix: string, response: () => Response): Handler {
  return (url) => (url.endsWith(`/${suffix}`) ? response() : undefined);
}

function callUrl(input: RequestInfo | URL) {
  return typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
}

function harness(
  handler: Handler,
  settings: {
    known?: Map<string, Uint8Array>;
    cached?: Map<string, Response>;
    storedRelease?: typeof release | null;
    token?: string;
  } = {},
) {
  const known = new Map<string, Uint8Array>(settings.known);
  if (settings.storedRelease !== null)
    known.set(
      'current',
      new TextEncoder().encode(JSON.stringify(settings.storedRelease ?? release)),
    );
  const cached = settings.cached ?? new Map<string, Response>();
  const fetcher = vi.fn<typeof fetch>(async (input, init) => {
    const url = input instanceof Request ? input.url : input.toString();
    const response = handler(url, init);
    if (!response) throw Error(`unexpected fetch ${url}`);
    return response;
  });
  const store: ReleaseStore = {
    get: async (key) => known.get(key)?.slice().buffer ?? null,
    put: async (key, value) => {
      known.set(key, value.slice());
    },
  };
  const cache: ReleaseCache = {
    match: async (request) => cached.get(request.url)?.clone(),
    put: async (request, response) => {
      cached.set(request.url, response.clone());
    },
  };
  const pending: Promise<unknown>[] = [];
  const context = {
    waitUntil: (promise: Promise<unknown>) => {
      pending.push(promise);
    },
  };
  return {
    cached,
    known,
    fetcher,
    cache,
    store,
    context,
    async request(
      path: string,
      options: { method?: string; range?: string; cf?: { timezone?: string } } = {},
    ) {
      const headers = new Headers({ Authorization: 'client-secret', Cookie: 'client-secret' });
      if (options.range) headers.set('Range', options.range);
      const request = new Request(`https://fujimod.com${path}`, {
        method: options.method ?? 'GET',
        headers,
      });
      if (options.cf) Object.defineProperty(request, 'cf', { value: options.cf });
      const response = await serveOta(request, cache, store, context, fetcher, settings.token);
      await Promise.all(pending);
      return response;
    },
    assetCalls(name: string) {
      return fetcher.mock.calls.filter(([input]) =>
        (input instanceof Request ? input.url : input.toString()).endsWith(`/${name}`),
      );
    },
    apiCalls() {
      return fetcher.mock.calls.filter(([input]) =>
        (input instanceof Request ? input.url : input.toString()).startsWith(
          'https://api.github.com',
        ),
      );
    },
  };
}

test('the release lookup and manifest answer from a five-minute edge cache', async () => {
  const clock = vi.spyOn(Date, 'now');
  const t0 = Date.parse('2026-11-01T00:00:00Z');
  clock.mockReturnValue(t0);
  const h = harness(
    combine(
      api(),
      download(manifestAsset, () => Response.json(manifest)),
    ),
  );
  const first = await h.request(`/ota/${firmware}/latest.json?nonce=1`);
  expect(first.status).toBe(200);
  expect(await first.json()).toEqual(manifest);
  expect(first.headers.get('content-type')).toBe('application/json');
  expect(first.headers.get('cache-control')).toBe('no-store');
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(manifestAsset)).toHaveLength(1);
  expect(h.cached.has(manifestUrl)).toBe(true);
  expect(h.cached.has(releaseUrl)).toBe(true);
  expect(h.cached.get(manifestUrl)?.headers.get(fetchedAt)).toBe(String(t0));
  expect(h.cached.get(releaseUrl)?.headers.get('cache-control')).toContain('max-age=31536000');
  expect(h.known.get(`ota/manifest/${firmware}`)).toBeDefined();

  await h.request(`/ota/${firmware}/latest.json`);
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(manifestAsset)).toHaveLength(1);
  clock.mockReturnValue(t0 + 4 * 60_000);
  await h.request(`/ota/${firmware}/latest.json`);
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(manifestAsset)).toHaveLength(1);
  clock.mockReturnValue(t0 + 5 * 60_000 + 1);
  await h.request(`/ota/${firmware}/latest.json`);
  expect(h.apiCalls()).toHaveLength(2);
  expect(h.assetCalls(manifestAsset)).toHaveLength(2);
  expect(h.cached.get(manifestUrl)?.headers.get(fetchedAt)).toBe(String(t0 + 5 * 60_000 + 1));

  const head = await h.request(`/ota/${firmware}/latest.json`, { method: 'HEAD' });
  expect(head.status).toBe(200);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe(String(JSON.stringify(manifest).length));
});

test('stale manifest and release copies answer while GitHub fails', async () => {
  const clock = vi.spyOn(Date, 'now');
  const t0 = Date.parse('2026-11-01T00:00:00Z');
  clock.mockReturnValue(t0);
  const h = harness(
    combine(
      api(),
      download(manifestAsset, () => Response.json(manifest)),
    ),
  );
  await h.request(`/ota/${firmware}/latest.json`);
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(manifestAsset)).toHaveLength(1);
  clock.mockReturnValue(t0 + 6 * 60_000);
  const failing = harness(
    combine(
      api(() => new Response(null, { status: 503 })),
      download(manifestAsset, () => new Response(null, { status: 503 })),
    ),
    { known: h.known, cached: h.cached },
  );
  const stale = await failing.request(`/ota/${firmware}/latest.json`);
  expect(stale.status).toBe(200);
  expect(await stale.json()).toEqual(manifest);
  expect(failing.apiCalls()).toHaveLength(1);
  expect(failing.assetCalls(manifestAsset)).toHaveLength(1);
  expect(failing.cached.get(manifestUrl)?.headers.get(fetchedAt)).toBe(String(t0 + 6 * 60_000));
  await failing.request(`/ota/${firmware}/latest.json`);
  expect(failing.apiCalls()).toHaveLength(1);
  expect(failing.assetCalls(manifestAsset)).toHaveLength(1);
});

test('a cold edge cache serves the KV manifest while GitHub fails', async () => {
  const known = new Map<string, Uint8Array>([
    [`ota/manifest/${firmware}`, new TextEncoder().encode(JSON.stringify(manifest))],
  ]);
  const h = harness(
    combine(
      api(() => new Response(null, { status: 503 })),
      download(manifestAsset, () => new Response(null, { status: 503 })),
    ),
    { known },
  );
  const stale = await h.request(`/ota/${firmware}/latest.json`);
  expect(stale.status).toBe(200);
  expect(await stale.json()).toEqual(manifest);
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(manifestAsset)).toHaveLength(1);
  expect(callUrl(h.assetCalls(manifestAsset)[0][0])).toContain(`/v2/${manifestAsset}`);
  expect(h.cached.has(manifestUrl)).toBe(true);
});

test('latest.json reports missing, broken and misplaced assets without a stale copy', async () => {
  const cases: [() => Response, number][] = [
    [() => new Response(null, { status: 404 }), 404],
    [() => new Response(null, { status: 503 }), 502],
    [() => new Response('not json'), 502],
    [() => Response.json({ firmware: 'other-cam', version: '1' }), 404],
    [() => Response.json({ version: '1' }), 404],
  ];
  for (const [upstream, status] of cases) {
    const h = harness(combine(api(), download(manifestAsset, upstream)));
    const response = await h.request(`/ota/${firmware}/latest.json`);
    expect(response.status).toBe(status);
    expect(response.headers.get('cache-control')).toBe('no-store');
    expect(h.known.has(`ota/manifest/${firmware}`)).toBe(false);
    const head = await h.request(`/ota/${firmware}/latest.json`, { method: 'HEAD' });
    expect(head.status).toBe(status);
    expect(await head.text()).toBe('');
  }
});

test('an empty or refused release stream explains itself', async () => {
  const cases: [Handler, number, string][] = [
    [() => Response.json([]), 404, 'No release has been published.'],
    [
      () => new Response(null, { status: 403 }),
      502,
      'GitHub release rate limit reached. Try again later.',
    ],
    [() => new Response(null, { status: 503 }), 502, 'Release upstream unavailable.'],
  ];
  for (const [upstream, status, message] of cases) {
    const h = harness(upstream, { storedRelease: null });
    const response = await h.request(`/ota/${firmware}/latest.json`);
    expect(response.status).toBe(status);
    expect(await response.text()).toBe(message);
    expect(h.apiCalls()).toHaveLength(1);
    const head = await h.request(`/ota/${firmware}/latest.json`, { method: 'HEAD' });
    expect(head.status).toBe(status);
    expect(await head.text()).toBe('');
  }
});

test('the GitHub token authenticates API lookups and never reaches asset requests', async () => {
  const h = harness(
    combine(
      api(),
      download(manifestAsset, () => Response.json(manifest)),
    ),
    { token: 'worker-secret' },
  );
  await h.request(`/ota/${firmware}/latest.json`);
  expect(h.apiCalls()).toHaveLength(1);
  const apiHeaders = new Headers(h.apiCalls()[0][1]?.headers);
  expect(apiHeaders.get('authorization')).toBe('Bearer worker-secret');
  expect(apiHeaders.has('cookie')).toBe(false);
  const assetHeaders = new Headers(h.assetCalls(manifestAsset)[0][1]?.headers);
  expect(assetHeaders.has('authorization')).toBe(false);
  expect(assetHeaders.has('cookie')).toBe(false);

  const anonymous = harness(
    combine(
      api(),
      download(manifestAsset, () => Response.json(manifest)),
    ),
  );
  await anonymous.request(`/ota/${firmware}/latest.json`);
  expect(new Headers(anonymous.apiCalls()[0][1]?.headers).has('authorization')).toBe(false);
});

test('store files are fetched whole once and cached immutably', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const h = harness(
    combine(
      api(),
      download(storeName, () => new Response(bytes)),
    ),
  );
  const first = await h.request(`/ota/store/${storeName}`);
  expect(first.status).toBe(200);
  expect(new Uint8Array(await first.arrayBuffer())).toEqual(bytes);
  expect(first.headers.get('content-type')).toBe('application/octet-stream');
  expect(first.headers.get('accept-ranges')).toBe('bytes');
  expect(first.headers.get('content-length')).toBe('5');
  expect(first.headers.get('cache-control')).toBe('public, max-age=31536000, immutable');
  expect(first.headers.get('x-content-type-options')).toBe('nosniff');
  expect(first.headers.get('content-security-policy')).toBeTruthy();
  expect(h.apiCalls()).toHaveLength(1);
  expect(h.assetCalls(storeName)).toHaveLength(1);
  expect(h.known.get(`ota/store/${storeName}`)).toEqual(bytes);
  expect(h.cached.has(storeUrl)).toBe(true);

  const second = await h.request(`/ota/store/${storeName}`);
  expect(new Uint8Array(await second.arrayBuffer())).toEqual(bytes);
  expect(h.assetCalls(storeName)).toHaveLength(1);
  const head = await h.request(`/ota/store/${storeName}`, { method: 'HEAD' });
  expect(head.status).toBe(200);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe('5');
});

test('ranges are answered from a whole cached asset and never forwarded', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const h = harness(
    combine(
      api(),
      download(storeName, () => new Response(bytes)),
    ),
  );
  const cold = await h.request(`/ota/store/${storeName}`, { range: 'bytes=1-3' });
  expect(cold.status).toBe(206);
  expect(cold.headers.get('content-range')).toBe('bytes 1-3/5');
  expect(cold.headers.get('content-length')).toBe('3');
  expect(new Uint8Array(await cold.arrayBuffer())).toEqual(new Uint8Array([2, 3, 4]));
  expect(h.assetCalls(storeName)).toHaveLength(1);
  expect(new Headers(h.assetCalls(storeName)[0][1]?.headers).has('range')).toBe(false);
  expect(h.cached.has(storeUrl)).toBe(true);
  expect(h.cached.get(storeUrl)?.headers.get('cache-control')).toContain('immutable');
  expect(h.known.get(`ota/store/${storeName}`)).toEqual(bytes);

  const warm = await h.request(`/ota/store/${storeName}`, { range: 'bytes=-2' });
  expect(warm.status).toBe(206);
  expect(warm.headers.get('content-range')).toBe('bytes 3-4/5');
  expect(new Uint8Array(await warm.arrayBuffer())).toEqual(new Uint8Array([4, 5]));
  expect(h.assetCalls(storeName)).toHaveLength(1);
  const head = await h.request(`/ota/store/${storeName}`, { method: 'HEAD', range: 'bytes=1-3' });
  expect(head.status).toBe(206);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe('3');
});

test('unsatisfiable and malformed ranges use the whole cached file', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const h = harness(
    combine(
      api(),
      download(storeName, () => new Response(bytes)),
    ),
  );
  const refused = await h.request(`/ota/store/${storeName}`, { range: 'bytes=5-' });
  expect(refused.status).toBe(416);
  expect(refused.headers.get('content-range')).toBe('bytes */5');
  expect(h.assetCalls(storeName)).toHaveLength(1);
  expect(h.known.get(`ota/store/${storeName}`)).toEqual(bytes);
  const read = async (range: string) => {
    const response = await h.request(`/ota/store/${storeName}`, { range });
    return {
      status: response.status,
      range: response.headers.get('content-range'),
      body: new Uint8Array(await response.arrayBuffer()),
    };
  };
  expect(await read('bytes=3-')).toEqual({
    status: 206,
    range: 'bytes 3-4/5',
    body: new Uint8Array([4, 5]),
  });
  expect(await read('bytes=0-99')).toEqual({ status: 206, range: 'bytes 0-4/5', body: bytes });
  for (const ignored of ['bytes=3-1', 'bytes=0-1,3-4', 'items=0-1', 'bytes=abc', 'bytes='])
    expect(await read(ignored)).toEqual({ status: 200, range: null, body: bytes });
  expect(h.assetCalls(storeName)).toHaveLength(1);

  const empty = harness(
    combine(
      api(),
      download(storeName, () => new Response(new Uint8Array())),
    ),
  );
  const whole = await empty.request(`/ota/store/${storeName}`);
  expect(whole.status).toBe(200);
  expect(whole.headers.get('content-length')).toBe('0');
  const none = await empty.request(`/ota/store/${storeName}`, { range: 'bytes=0-' });
  expect(none.status).toBe(416);
  expect(none.headers.get('content-range')).toBe('bytes */0');
});

test('stored store bytes answer while GitHub refuses', async () => {
  const bytes = new Uint8Array([5, 6, 7, 8]);
  const known = new Map([[`ota/store/${storeName}`, bytes]]);
  for (const status of [404, 503]) {
    const h = harness(
      combine(
        api(() => new Response(null, { status: 503 })),
        download(storeName, () => new Response(null, { status })),
      ),
      { known },
    );
    const response = await h.request(`/ota/store/${storeName}`);
    expect(response.status).toBe(200);
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
    const ranged = await h.request(`/ota/store/${storeName}`, { range: 'bytes=-2' });
    expect(ranged.status).toBe(206);
    expect(new Uint8Array(await ranged.arrayBuffer())).toEqual(new Uint8Array([7, 8]));
  }
  for (const [status, expected] of [
    [404, 404],
    [503, 502],
  ] as const) {
    const h = harness(
      combine(
        api(() => new Response(null, { status: 503 })),
        download(storeName, () => new Response(null, { status })),
      ),
    );
    expect((await h.request(`/ota/store/${storeName}`)).status).toBe(expected);
  }
  const limited = harness(
    api(() => new Response(null, { status: 403 })),
    {
      storedRelease: null,
    },
  );
  const response = await limited.request(`/ota/store/${storeName}`);
  expect(response.status).toBe(502);
  expect(await response.text()).toBe('GitHub release rate limit reached. Try again later.');
});

test('malformed OTA paths and write methods never reach GitHub', async () => {
  const h = harness(() => undefined);
  for (const path of [
    '/ota',
    '/ota/',
    '/ota/release.json',
    `/ota/${firmware}/latest.json/extra`,
    `/ota/${firmware}/other.json`,
    '/ota/-bad/latest.json',
    '/ota/store/ba7816bf.8f0',
    '/ota/store/BA7816BF.8F0/extra',
    '/ota/store/%2e%2e%2fsecret',
    '/ota/store/BA7816BF',
  ])
    expect((await h.request(path)).status).toBe(404);
  expect((await h.request(`/ota/${firmware}/latest.json`, { method: 'POST' })).status).toBe(405);
  expect((await h.request(`/ota/store/${storeName}`, { method: 'PUT' })).status).toBe(405);
  expect(h.fetcher).not.toHaveBeenCalled();
});

test('timezone reports the Cloudflare request zone as JSON', async () => {
  const h = harness(() => undefined);
  const response = await h.request('/ota/timezone', { cf: { timezone: 'America/New_York' } });
  expect(response.status).toBe(200);
  expect(response.headers.get('content-type')).toBe('application/json');
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(await response.json()).toEqual({ timezone: 'America/New_York' });
  expect(await (await h.request('/ota/timezone')).json()).toEqual({ timezone: null });
  expect((await h.request('/ota/timezone', { method: 'POST' })).status).toBe(405);
  const head = await h.request('/ota/timezone', { method: 'HEAD', cf: { timezone: 'UTC' } });
  expect(head.status).toBe(200);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe('18');
  expect(h.fetcher).not.toHaveBeenCalled();
});

test('the Worker sends OTA paths to the proxy and leaves static paths to assets', async () => {
  const served: string[] = [];
  const cache: ReleaseCache = {
    match: async () => undefined,
    put: async () => {},
  };
  vi.stubGlobal('caches', {
    open: async (name: string) => {
      served.push(name);
      return cache;
    },
  });
  try {
    const env = {
      ASSETS: {
        fetch: async () => {
          served.push('assets');
          return new Response('static');
        },
      },
      RELEASES: {
        get: async () => null,
        put: async () => {},
      } as ReleaseStore,
      AUTH_DB: {} as D1Database,
      BETTER_AUTH_SECRET: 'test-secret-test-secret-test-secret',
      GOOGLE_CLIENT_ID: 'test-client-id',
      GOOGLE_CLIENT_SECRET: 'test-client-secret',
    };
    const zone = await worker.fetch(new Request('https://fujimod.com/ota/timezone'), env, {
      waitUntil() {},
    });
    expect(await zone.json()).toEqual({ timezone: null });
    expect(served).toEqual(['fujimod-ota']);
    const internal = await worker.fetch(new Request('https://fujimod.com/ota/release.json'), env, {
      waitUntil() {},
    });
    expect(internal.status).toBe(404);
    expect(served).toEqual(['fujimod-ota', 'fujimod-ota']);
    const page = await worker.fetch(new Request('https://fujimod.com/'), env, { waitUntil() {} });
    expect(await page.text()).toBe('static');
  } finally {
    vi.unstubAllGlobals();
  }
});
