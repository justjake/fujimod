import { test, expect, vi } from 'vite-plus/test';
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

function harness(
  handler: Handler,
  options: { known?: Map<string, Uint8Array>; storedRelease?: typeof release | null } = {},
) {
  const known = new Map<string, Uint8Array>(options.known);
  if (options.storedRelease !== null)
    known.set(
      'current',
      new TextEncoder().encode(JSON.stringify(options.storedRelease ?? release)),
    );
  const cached = new Map<string, Response>();
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
      const headers = new Headers({ Authorization: 'secret', Cookie: 'secret' });
      if (options.range) headers.set('Range', options.range);
      const request = new Request(`https://fujimod.com${path}`, {
        method: options.method ?? 'GET',
        headers,
      });
      if (options.cf) Object.defineProperty(request, 'cf', { value: options.cf });
      const response = await serveOta(request, cache, store, context, fetcher);
      await Promise.all(pending);
      return response;
    },
    assetCalls(name: string) {
      return fetcher.mock.calls.filter(([input]) =>
        (input instanceof Request ? input.url : input.toString()).endsWith(`/${name}`),
      );
    },
  };
}

function api(handler?: Handler) {
  return (url: string, init?: RequestInit) =>
    url.startsWith('https://api.github.com')
      ? (handler?.(url, init) ?? Response.json(releaseList()))
      : undefined;
}

test('latest.json comes from the current release on every request and is never stored', async () => {
  const h = harness(
    (url, init) =>
      api()(url, init) ??
      (url === 'https://github.com/justjake/fujimod/releases/download/v2/x70-110-latest.json'
        ? Response.json(manifest)
        : undefined),
  );
  const response = await h.request('/ota/x70-110/latest.json?nonce=1');
  expect(response.status).toBe(200);
  expect(await response.json()).toEqual(manifest);
  expect(response.headers.get('content-type')).toBe('application/json');
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(h.cached.size).toBe(0);
  expect(h.known.has(`ota/store/${storeName}`)).toBe(false);
  await h.request('/ota/x70-110/latest.json');
  expect(h.assetCalls('x70-110-latest.json')).toHaveLength(2);
  for (const [, init] of h.fetcher.mock.calls) {
    expect(new Headers(init?.headers).has('authorization')).toBe(false);
    expect(new Headers(init?.headers).has('cookie')).toBe(false);
    expect(init?.redirect ?? 'follow').toBe('follow');
  }
  const head = await h.request('/ota/x70-110/latest.json', { method: 'HEAD' });
  expect(head.status).toBe(200);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe(String(JSON.stringify(manifest).length));
});

test('latest.json routes missing, broken and misplaced manifests to errors', async () => {
  const cases: [Response, number][] = [
    [new Response(null, { status: 404 }), 404],
    [new Response(null, { status: 503 }), 502],
    [new Response('not json'), 502],
    [Response.json({ firmware: 'other-cam', version: '1' }), 404],
    [Response.json({ version: '1' }), 404],
  ];
  for (const [upstream, status] of cases) {
    const h = harness(
      (url, init) =>
        api()(url, init) ?? (url.endsWith('/x70-110-latest.json') ? upstream : undefined),
    );
    const response = await h.request('/ota/x70-110/latest.json');
    expect(response.status).toBe(status);
    expect(response.headers.get('cache-control')).toBe('no-store');
  }
});

test('store files are served once and cached immutably', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const h = harness(
    (url, init) =>
      api()(url, init) ?? (url.endsWith(`/${storeName}`) ? new Response(bytes) : undefined),
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
  const second = await h.request(`/ota/store/${storeName}`);
  expect(new Uint8Array(await second.arrayBuffer())).toEqual(bytes);
  expect(h.assetCalls(storeName)).toHaveLength(1);
  expect(h.known.get(`ota/store/${storeName}`)).toEqual(bytes);
  expect(h.cached.has(`https://fujimod.com/ota/store/${storeName}`)).toBe(true);
  const head = await h.request(`/ota/store/${storeName}`, { method: 'HEAD' });
  expect(head.status).toBe(200);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe('5');
});

test('range requests pass through before a store file is cached', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const h = harness((url, init) => {
    const base = api()(url, init);
    if (base) return base;
    if (!url.endsWith(`/${storeName}`)) return;
    const requested = new Headers(init?.headers).get('range');
    if (requested === 'bytes=1-3')
      return new Response(bytes.subarray(1, 4), {
        status: 206,
        headers: { 'Content-Range': 'bytes 1-3/5' },
      });
    return new Response(bytes);
  });
  const ranged = await h.request(`/ota/store/${storeName}`, { range: 'bytes=1-3' });
  expect(ranged.status).toBe(206);
  expect(ranged.headers.get('content-range')).toBe('bytes 1-3/5');
  expect(ranged.headers.get('content-length')).toBe('3');
  expect(new Uint8Array(await ranged.arrayBuffer())).toEqual(new Uint8Array([2, 3, 4]));
  expect(new Headers(h.assetCalls(storeName)[0][1]?.headers).get('range')).toBe('bytes=1-3');
  expect(h.cached.size).toBe(0);
  expect(h.known.has(`ota/store/${storeName}`)).toBe(false);
  const full = await h.request(`/ota/store/${storeName}`);
  expect(full.status).toBe(200);
  const warm = await h.request(`/ota/store/${storeName}`, { range: 'bytes=1-3' });
  expect(warm.status).toBe(206);
  expect(warm.headers.get('content-range')).toBe('bytes 1-3/5');
  expect(new Uint8Array(await warm.arrayBuffer())).toEqual(new Uint8Array([2, 3, 4]));
  expect(h.assetCalls(storeName)).toHaveLength(2);
  const head = await h.request(`/ota/store/${storeName}`, {
    method: 'HEAD',
    range: 'bytes=1-3',
  });
  expect(head.status).toBe(206);
  expect(await head.text()).toBe('');
  expect(head.headers.get('content-length')).toBe('3');
});

test('range forms are clamped, ignored or refused as the standard requires', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  let full = 0;
  const h = harness((url, init) => {
    const base = api()(url, init);
    if (base) return base;
    if (!url.endsWith(`/${storeName}`)) return;
    full++;
    return new Response(bytes);
  });
  await h.request(`/ota/store/${storeName}`);
  const read = async (range: string) => {
    const response = await h.request(`/ota/store/${storeName}`, { range });
    return {
      status: response.status,
      range: response.headers.get('content-range'),
      body: new Uint8Array(await response.arrayBuffer()),
    };
  };
  expect(await read('bytes=-2')).toEqual({
    status: 206,
    range: 'bytes 3-4/5',
    body: new Uint8Array([4, 5]),
  });
  expect(await read('bytes=3-')).toEqual({
    status: 206,
    range: 'bytes 3-4/5',
    body: new Uint8Array([4, 5]),
  });
  expect(await read('bytes=0-99')).toEqual({
    status: 206,
    range: 'bytes 0-4/5',
    body: bytes,
  });
  expect(await read('bytes=5-')).toEqual({
    status: 416,
    range: 'bytes */5',
    body: new Uint8Array(),
  });
  for (const ignored of ['bytes=3-1', 'bytes=0-1,3-4', 'items=0-1', 'bytes=abc', 'bytes='])
    expect(await read(ignored)).toEqual({ status: 200, range: null, body: bytes });
  expect(full).toBe(1);

  const empty = harness(
    (url, init) =>
      api()(url, init) ??
      (url.endsWith(`/${storeName}`) ? new Response(new Uint8Array()) : undefined),
  );
  const whole = await empty.request(`/ota/store/${storeName}`);
  expect(whole.status).toBe(200);
  expect(whole.headers.get('content-length')).toBe('0');
  const refused = await empty.request(`/ota/store/${storeName}`, { range: 'bytes=0-' });
  expect(refused.status).toBe(416);
  expect(refused.headers.get('content-range')).toBe('bytes */0');
});

test('an upstream 416 is relayed with its Content-Range', async () => {
  const h = harness(
    (url, init) =>
      api()(url, init) ??
      (url.endsWith(`/${storeName}`)
        ? new Response(null, { status: 416, headers: { 'Content-Range': 'bytes */5' } })
        : undefined),
  );
  const response = await h.request(`/ota/store/${storeName}`, { range: 'bytes=9-' });
  expect(response.status).toBe(416);
  expect(response.headers.get('content-range')).toBe('bytes */5');
});

test('stored store bytes answer while GitHub refuses', async () => {
  const bytes = new Uint8Array([5, 6, 7, 8]);
  const known = new Map([[`ota/store/${storeName}`, bytes]]);
  for (const status of [404, 503]) {
    const h = harness(
      (url, init) =>
        api()(url, init) ??
        (url.endsWith(`/${storeName}`) ? new Response(null, { status }) : undefined),
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
      (url, init) =>
        api()(url, init) ??
        (url.endsWith(`/${storeName}`) ? new Response(null, { status }) : undefined),
    );
    expect((await h.request(`/ota/store/${storeName}`)).status).toBe(expected);
  }
});

test('an empty or rate-limited release stream explains itself', async () => {
  for (const [upstream, status, message] of [
    [Response.json([]), 404, 'No release has been published.'],
    [
      new Response(null, { status: 403 }),
      502,
      'GitHub release rate limit reached. Try again later.',
    ],
    [new Response(null, { status: 503 }), 502, 'Release upstream unavailable.'],
  ] as const) {
    const h = harness(() => upstream.clone(), { storedRelease: null });
    const response = await h.request('/ota/x70-110/latest.json');
    expect(response.status).toBe(status);
    expect(await response.text()).toBe(message);
    expect(h.fetcher).toHaveBeenCalledOnce();
    const head = await h.request(`/ota/store/${storeName}`, { method: 'HEAD' });
    expect(head.status).toBe(status);
    expect(await head.text()).toBe('');
  }
});

test('malformed OTA paths and write methods never reach the release', async () => {
  const h = harness(() => undefined);
  for (const path of [
    '/ota',
    '/ota/',
    '/ota/x70-110/latest.json/extra',
    '/ota/x70-110/other.json',
    '/ota/-bad/latest.json',
    '/ota/store/ba7816bf.8f0',
    '/ota/store/BA7816BF.8F0/extra',
    '/ota/store/%2e%2e%2fsecret',
    '/ota/store/BA7816BF',
  ])
    expect((await h.request(path)).status).toBe(404);
  expect((await h.request('/ota/x70-110/latest.json', { method: 'POST' })).status).toBe(405);
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
    };
    const zone = await worker.fetch(new Request('https://fujimod.com/ota/timezone'), env, {
      waitUntil() {},
    });
    expect(await zone.json()).toEqual({ timezone: null });
    expect(served).toEqual(['fujimod-ota']);
    const page = await worker.fetch(new Request('https://fujimod.com/'), env, { waitUntil() {} });
    expect(await page.text()).toBe('static');
  } finally {
    vi.unstubAllGlobals();
  }
});
