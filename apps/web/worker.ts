import { runtimeHeaders } from './src/security.ts';
import { releaseRoute, validateReleaseInfo, type ReleaseInfo } from './src/patch/releases.ts';
import { createAuth } from './src/auth/runtime.ts';
import type { AuthEnv } from './src/auth/options.ts';
import { serveDeviceToken, serveGoogleToken, type ProviderTokenApi } from './src/devices/routes.ts';
import { serveDownloadEvent } from './src/downloads/routes.ts';
import { byteRange, otaRoute } from './src/ota/routes.ts';

const repository = 'justjake/fujimod';
const api = `https://api.github.com/repos/${repository}/releases`;
const headers = { Accept: 'application/vnd.github+json', 'User-Agent': 'fujimod-web' };
const downloads = `https://github.com/${repository}/releases/download`;
interface GitHubRelease {
  tag_name: string;
  name: string | null;
  draft: boolean;
  prerelease: boolean;
  published_at: string | null;
  assets: { name: string; state: string }[];
}
interface Context {
  waitUntil(promise: Promise<unknown>): void;
}
interface Assets {
  fetch(request: Request): Promise<Response>;
}
/** Requests reaching the Worker carry Cloudflare's request metadata. */
interface CloudflareRequest extends Request {
  cf?: { timezone?: string };
}
export interface ReleaseCache {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<unknown>;
}
/** Global copies of the current release and its files. Requests answer from
 * here, so GitHub's shared unauthenticated limit or an outage never reaches a
 * visitor once a release has been seen. */
export interface ReleaseStore {
  get(key: string, type: 'arrayBuffer'): Promise<ArrayBuffer | null>;
  put(key: string, value: Uint8Array): Promise<unknown>;
}
interface Env extends AuthEnv {
  ASSETS: Assets;
  RELEASES: ReleaseStore;
  GITHUB_TOKEN?: string;
}
interface AuthHandler {
  handler(request: Request): Promise<Response>;
  api: ProviderTokenApi;
}
type AuthFactory = (env: AuthEnv, baseURL: string) => AuthHandler;
const currentKey = 'current';
const storeBytesKey = (name: string) => `ota/store/${name}`;
const manifestBytesKey = (firmware: string) => `ota/manifest/${firmware}`;
const site = 'https://fujimod.com';
const releaseCacheUrl = `${site}/ota/release.json`;
const manifestCacheUrl = (firmware: string) => `${site}/ota/${firmware}/latest.json`;
const storeCacheUrl = (name: string) => `${site}/ota/store/${name}`;
/** Camera update metadata is read from GitHub at most this often per edge. */
const otaFresh = 300_000;
const fetchedAtHeader = 'x-fujimod-fetched-at';
class UpstreamError extends Error {
  readonly status: number;
  constructor(status: number) {
    super('Release upstream unavailable.');
    this.status = status;
  }
}
async function currentRelease(
  fetcher: typeof fetch,
  token?: string,
): Promise<ReleaseInfo | undefined> {
  let newest: GitHubRelease | undefined;
  const requestHeaders = token ? { ...headers, Authorization: `Bearer ${token}` } : headers;
  // GitHub orders by creation rather than publication time. Inspect every page,
  // including prereleases and releases published from older tags.
  for (let page = 1; ; page++) {
    const response = await fetcher(`${api}?per_page=100&page=${page}`, {
      headers: requestHeaders,
    });
    if (!response.ok) throw new UpstreamError(response.status);
    const releases = (await response.json()) as GitHubRelease[];
    if (!Array.isArray(releases)) throw new UpstreamError(502);
    for (const release of releases) {
      if (
        !release.draft &&
        release.published_at &&
        release.assets.some(
          (asset) => asset.name === 'patches.json' && asset.state === 'uploaded',
        ) &&
        (!newest || Date.parse(release.published_at) > Date.parse(newest.published_at!))
      )
        newest = release;
    }
    if (!response.headers.get('link')?.includes('rel="next"')) break;
  }
  return newest
    ? {
        tag: newest.tag_name,
        name: newest.name || newest.tag_name,
        published_at: newest.published_at!,
        prerelease: newest.prerelease,
      }
    : undefined;
}
async function saveChanged(store: ReleaseStore, key: string, value: Uint8Array) {
  const known = await store.get(key, 'arrayBuffer');
  if (known && equalBytes(new Uint8Array(known), value)) return;
  await store.put(key, value);
}
/** Fetch or refresh the release metadata shared by every proxy route. */
async function refreshCurrent(
  store: ReleaseStore,
  fetcher: typeof fetch,
  token?: string,
): Promise<Uint8Array<ArrayBuffer> | undefined> {
  const release = await currentRelease(fetcher, token);
  if (!release) return undefined;
  const text = new TextEncoder().encode(JSON.stringify(release));
  await saveChanged(store, currentKey, text);
  return text;
}
function releaseAsset(tag: string, name: string) {
  return `${downloads}/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;
}
/** Stored OTA responses carry their fetch time. The edge keeps them for a
 * year; code treats them as fresh for five minutes, so an origin outage still
 * has a copy to answer with. */
interface CachedBytes {
  bytes: Uint8Array<ArrayBuffer>;
  fetchedAt: number;
}
async function readCached(cache: ReleaseCache, url: string): Promise<CachedBytes | undefined> {
  const response = await cache.match(new Request(url, { method: 'GET' }));
  if (!response) return undefined;
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    fetchedAt: Number(response.headers.get(fetchedAtHeader)),
  };
}
function edgeEntry(bytes: Uint8Array<ArrayBuffer>, contentType: string): Response {
  return new Response(bytes, {
    headers: {
      'Content-Type': contentType,
      'Cache-Control': 'public, max-age=31536000',
      [fetchedAtHeader]: String(Date.now()),
    },
  });
}
function isFresh(entry: CachedBytes | undefined): boolean {
  return entry !== undefined && Date.now() - entry.fetchedAt < otaFresh;
}
function releaseTag(bytes: Uint8Array<ArrayBuffer>): string | undefined {
  try {
    return validateReleaseInfo(JSON.parse(new TextDecoder().decode(bytes))).tag;
  } catch {
    return undefined;
  }
}
function equalBytes(a: Uint8Array, b: Uint8Array) {
  return a.length === b.length && a.every((byte, index) => byte === b[index]);
}
function failure(status: number, message: string) {
  return new Response(message, {
    status,
    headers: { ...runtimeHeaders, 'Cache-Control': 'no-store' },
  });
}
function methodNotAllowed() {
  return new Response(null, {
    status: 405,
    headers: { Allow: 'GET, HEAD', 'Cache-Control': 'no-store' },
  });
}
/** Only successful release responses enter the edge cache or the store; credentials are never forwarded. */
export async function serveRelease(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch = fetch,
  token?: string,
): Promise<Response> {
  const url = new URL(request.url),
    route = releaseRoute(url.pathname);
  if (!route) return failure(404, 'Release route not found.');
  if (request.method !== 'GET' && request.method !== 'HEAD') return methodNotAllowed();
  url.search = '';
  const key = new Request(url, { method: 'GET' });
  const head = (response: Response) =>
    request.method === 'HEAD'
      ? new Response(null, { status: response.status, headers: response.headers })
      : response;
  const cached = await cache.match(key);
  if (cached) return head(cached);
  try {
    let body: Uint8Array<ArrayBuffer>;
    if (route.kind === 'current') {
      // Answer from the stored copy and refresh it in the background; only the
      // first lookup after a deployment waits on GitHub.
      const refresh = () => refreshCurrent(store, fetcher, token);
      const known = await store.get(currentKey, 'arrayBuffer');
      if (known) {
        context.waitUntil(refresh().catch(() => undefined));
        body = new Uint8Array(known);
      } else {
        const fetched = await refresh();
        if (!fetched) return head(failure(404, 'No patch release has been published.'));
        body = fetched;
      }
    } else {
      const assetKey = `asset/${route.tag}/${route.file}`;
      const upstream = await fetcher(releaseAsset(route.tag, route.file), {
        headers: { 'User-Agent': 'fujimod-web' },
        redirect: 'follow',
      }).catch(() => undefined);
      if (upstream?.ok) {
        body = new Uint8Array(await upstream.arrayBuffer());
        context.waitUntil(saveChanged(store, assetKey, body));
      } else {
        const known = await store.get(assetKey, 'arrayBuffer');
        if (!known)
          return head(failure(upstream?.status === 404 ? 404 : 502, 'Release asset unavailable.'));
        body = new Uint8Array(known);
      }
    }
    const contentType =
      route.kind === 'current' || route.file.endsWith('.json')
        ? 'application/json'
        : route.file.endsWith('.md')
          ? 'text/markdown; charset=utf-8'
          : 'application/octet-stream';
    const response = new Response(body, { headers: { 'Content-Type': contentType } });
    for (const [name, value] of Object.entries(runtimeHeaders)) response.headers.set(name, value);
    response.headers.set('X-Content-Type-Options', 'nosniff');
    response.headers.set(
      'Cache-Control',
      `public, max-age=0, s-maxage=${route.kind === 'current' ? 300 : 31536000}`,
    );
    context.waitUntil(cache.put(key, response.clone()));
    return head(response);
  } catch (error) {
    return head(
      failure(
        502,
        error instanceof UpstreamError && [403, 429].includes(error.status)
          ? 'GitHub release rate limit reached. Try again later.'
          : 'Release upstream unavailable.',
      ),
    );
  }
}
/** The current release's tag. A five-minute edge copy answers camera requests;
 * a KV copy covers an origin outage; only a cold cache calls GitHub's API. */
async function currentTag(
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch,
  token?: string,
): Promise<string | undefined> {
  const cached = await readCached(cache, releaseCacheUrl);
  if (cached && isFresh(cached)) {
    const tag = releaseTag(cached.bytes);
    if (tag) return tag;
  }
  let stale = cached?.bytes;
  if (!stale) {
    const known = await store.get(currentKey, 'arrayBuffer');
    if (known) stale = new Uint8Array(known);
  }
  try {
    const release = await currentRelease(fetcher, token);
    if (!release) return undefined;
    const bytes = new TextEncoder().encode(JSON.stringify(release));
    context.waitUntil(
      cache.put(new Request(releaseCacheUrl), edgeEntry(bytes, 'application/json')),
    );
    context.waitUntil(saveChanged(store, currentKey, bytes));
    return release.tag;
  } catch (error) {
    const tag = stale ? releaseTag(stale) : undefined;
    if (stale && tag) {
      context.waitUntil(
        cache.put(new Request(releaseCacheUrl), edgeEntry(stale, 'application/json')),
      );
      return tag;
    }
    throw error;
  }
}
function assetRequest(tag: string, name: string, fetcher: typeof fetch) {
  return fetcher(releaseAsset(tag, name), {
    headers: { 'User-Agent': 'fujimod-web' },
    redirect: 'follow',
  }).catch(() => undefined);
}
function storeHeaders(length: number) {
  const result = new Headers({
    'Content-Type': 'application/octet-stream',
    'Content-Length': String(length),
    'Accept-Ranges': 'bytes',
    'Cache-Control': 'public, max-age=31536000, immutable',
    'X-Content-Type-Options': 'nosniff',
  });
  for (const [name, value] of Object.entries(runtimeHeaders)) result.set(name, value);
  return result;
}
function rangeNotSatisfiable(size: number) {
  const result = storeHeaders(0);
  result.set('Content-Range', `bytes */${size}`);
  result.set('Cache-Control', 'no-store');
  return new Response(null, { status: 416, headers: result });
}
/** Answer one store file from its full bytes, honoring a single `bytes=` range. */
function storeResponse(
  method: string,
  bytes: Uint8Array<ArrayBuffer>,
  rangeHeader: string | null,
): Response {
  const range = byteRange(rangeHeader, bytes.length);
  if (range === 'unsatisfiable') return rangeNotSatisfiable(bytes.length);
  const body = range ? bytes.subarray(range.start, range.end + 1) : bytes;
  const result = storeHeaders(body.length);
  if (range) result.set('Content-Range', `bytes ${range.start}-${range.end}/${bytes.length}`);
  return new Response(method === 'HEAD' ? null : body, {
    status: range ? 206 : 200,
    headers: result,
  });
}
function manifestResponse(method: string, bytes: Uint8Array<ArrayBuffer>): Response {
  const result = new Headers({
    'Content-Type': 'application/json',
    'Content-Length': String(bytes.length),
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  for (const [name, value] of Object.entries(runtimeHeaders)) result.set(name, value);
  return new Response(method === 'HEAD' ? null : bytes, { headers: result });
}
function manifestProblem(
  bytes: Uint8Array<ArrayBuffer>,
  firmware: string,
): 'invalid' | 'firmware' | undefined {
  let manifest: unknown;
  try {
    manifest = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return 'invalid';
  }
  if (
    !manifest ||
    typeof manifest !== 'object' ||
    (manifest as { firmware?: unknown }).firmware !== firmware
  )
    return 'firmware';
}
async function serveManifest(
  request: Request,
  firmware: string,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch,
  token?: string,
): Promise<Response> {
  const key = manifestCacheUrl(firmware);
  const cached = await readCached(cache, key);
  if (cached && isFresh(cached)) return manifestResponse(request.method, cached.bytes);
  let stale = cached?.bytes;
  if (!stale) {
    const known = await store.get(manifestBytesKey(firmware), 'arrayBuffer');
    if (known) stale = new Uint8Array(known);
  }
  const fallback = () => {
    if (!stale) return undefined;
    context.waitUntil(cache.put(new Request(key), edgeEntry(stale, 'application/json')));
    return manifestResponse(request.method, stale);
  };
  try {
    const tag = await currentTag(cache, store, context, fetcher, token);
    if (!tag) return fallback() ?? failure(404, 'No release has been published.');
    const upstream = await assetRequest(tag, `${firmware}-latest.json`, fetcher);
    if (upstream?.status !== 200)
      return (
        fallback() ??
        (upstream?.status === 404
          ? failure(404, 'No release manifest has been published for this firmware.')
          : failure(502, 'Release upstream unavailable.'))
      );
    const bytes = new Uint8Array(await upstream.arrayBuffer());
    const problem = manifestProblem(bytes, firmware);
    if (problem)
      return (
        fallback() ??
        (problem === 'invalid'
          ? failure(502, 'The release manifest is not valid JSON.')
          : failure(404, 'No release manifest has been published for this firmware.'))
      );
    context.waitUntil(cache.put(new Request(key), edgeEntry(bytes, 'application/json')));
    context.waitUntil(saveChanged(store, manifestBytesKey(firmware), bytes));
    return manifestResponse(request.method, bytes);
  } catch (error) {
    const response = fallback();
    if (response) return response;
    throw error;
  }
}
async function serveStore(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  name: string,
  fetcher: typeof fetch,
  token?: string,
): Promise<Response> {
  const range = request.headers.get('Range');
  const key = storeCacheUrl(name);
  const cached = await cache.match(new Request(key, { method: 'GET' }));
  if (cached)
    return storeResponse(request.method, new Uint8Array(await cached.arrayBuffer()), range);
  let problem: unknown;
  let bytes: Uint8Array<ArrayBuffer> | undefined;
  try {
    const tag = await currentTag(cache, store, context, fetcher, token);
    if (!tag) throw new UpstreamError(404);
    const upstream = await assetRequest(tag, name, fetcher);
    if (upstream?.status !== 200) throw new UpstreamError(upstream?.status === 404 ? 404 : 502);
    bytes = new Uint8Array(await upstream.arrayBuffer());
  } catch (error) {
    problem = error;
  }
  if (bytes) {
    context.waitUntil(
      cache.put(
        new Request(key),
        new Response(bytes, {
          headers: {
            'Content-Type': 'application/octet-stream',
            'Cache-Control': 'public, max-age=31536000, immutable',
          },
        }),
      ),
    );
    context.waitUntil(saveChanged(store, storeBytesKey(name), bytes));
    return storeResponse(request.method, bytes, range);
  }
  const known = await store.get(storeBytesKey(name), 'arrayBuffer');
  if (known) return storeResponse(request.method, new Uint8Array(known), range);
  return failure(
    problem instanceof UpstreamError && problem.status === 404 ? 404 : 502,
    problem instanceof UpstreamError && [403, 429].includes(problem.status)
      ? 'GitHub release rate limit reached. Try again later.'
      : problem instanceof UpstreamError && problem.status === 404
        ? 'Store file unavailable.'
        : 'Release upstream unavailable.',
  );
}
/** The camera's clock setup asks for the time zone of the request's location. */
function serveTimezone(request: Request): Response {
  if (request.method !== 'GET' && request.method !== 'HEAD') return methodNotAllowed();
  const timezone = (request as CloudflareRequest).cf?.timezone ?? null;
  const bytes = new TextEncoder().encode(JSON.stringify({ timezone }));
  const result = new Headers({
    'Content-Type': 'application/json',
    'Content-Length': String(bytes.length),
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  for (const [name, value] of Object.entries(runtimeHeaders)) result.set(name, value);
  return new Response(request.method === 'HEAD' ? null : bytes, { headers: result });
}
function headOnly(request: Request, response: Response): Response {
  if (request.method !== 'HEAD' || !response.body) return response;
  return new Response(null, { status: response.status, headers: response.headers });
}
/** Serve `/ota/...` from the current GitHub release. Camera reads answer from
 * a five-minute edge copy; store files are cached whole and sliced locally. */
export async function serveOta(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch = fetch,
  token?: string,
): Promise<Response> {
  return headOnly(request, await serveOtaPath(request, cache, store, context, fetcher, token));
}
async function serveOtaPath(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch,
  token?: string,
): Promise<Response> {
  const path = new URL(request.url).pathname;
  if (path === '/ota/timezone') return serveTimezone(request);
  if (path.startsWith('/ota/')) {
    const route = otaRoute(path);
    if (!route) return failure(404, 'OTA route not found.');
    if (request.method !== 'GET' && request.method !== 'HEAD') return methodNotAllowed();
    try {
      return route.kind === 'manifest'
        ? await serveManifest(request, route.firmware, cache, store, context, fetcher, token)
        : await serveStore(request, cache, store, context, route.name, fetcher, token);
    } catch (error) {
      return failure(
        502,
        error instanceof UpstreamError && [403, 429].includes(error.status)
          ? 'GitHub release rate limit reached. Try again later.'
          : 'Release upstream unavailable.',
      );
    }
  }
  return failure(404, 'OTA route not found.');
}
export async function serveRequest(
  request: Request,
  env: Env,
  context: Context,
  makeAuth: AuthFactory = createAuth,
): Promise<Response> {
  const url = new URL(request.url);
  if (url.pathname === '/api/auth/device/token') {
    const auth = makeAuth(env, url.origin);
    return serveDeviceToken(request, env.AUTH_DB, (incoming) => auth.handler(incoming));
  }
  if (url.pathname === '/api/auth' || url.pathname.startsWith('/api/auth/'))
    return makeAuth(env, url.origin).handler(request);
  if (url.pathname === '/api/devices/google-token')
    return serveGoogleToken(request, env.AUTH_DB, makeAuth(env, url.origin).api);
  if (url.pathname === '/api/downloads') return serveDownloadEvent(request, env.AUTH_DB);
  if (url.pathname.startsWith('/ota/'))
    return serveOta(
      request,
      await caches.open('fujimod-ota'),
      env.RELEASES,
      context,
      fetch,
      env.GITHUB_TOKEN,
    );
  if (!url.pathname.startsWith('/releases/')) return env.ASSETS.fetch(request);
  return serveRelease(
    request,
    await caches.open('fujimod-releases'),
    env.RELEASES,
    context,
    fetch,
    env.GITHUB_TOKEN,
  );
}

export default { fetch: serveRequest };
