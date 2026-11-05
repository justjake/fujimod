import { runtimeHeaders } from './src/security.ts';
import { releaseRoute, validateReleaseInfo, type ReleaseInfo } from './src/patch/releases.ts';
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
const currentKey = 'current';
const storeKey = (name: string) => `ota/store/${name}`;
const releaseRefresh = 300_000;
let releaseRefreshedAt = 0;
class UpstreamError extends Error {
  readonly status: number;
  constructor(status: number) {
    super('Release upstream unavailable.');
    this.status = status;
  }
}
async function currentRelease(fetcher: typeof fetch): Promise<ReleaseInfo | undefined> {
  let newest: GitHubRelease | undefined;
  // GitHub orders by creation rather than publication time. Inspect every page,
  // including prereleases and releases published from older tags.
  for (let page = 1; ; page++) {
    const response = await fetcher(`${api}?per_page=100&page=${page}`, { headers });
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
): Promise<Uint8Array<ArrayBuffer> | undefined> {
  const release = await currentRelease(fetcher);
  if (!release) return undefined;
  const text = new TextEncoder().encode(JSON.stringify(release));
  await saveChanged(store, currentKey, text);
  return text;
}
function releaseAsset(tag: string, name: string) {
  return `${downloads}/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;
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
      const refresh = () => refreshCurrent(store, fetcher);
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
/** The current release's tag, from the stored copy while a background refresh
 * follows GitHub. The manifest and the store both live in that release. */
async function currentTag(
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch,
): Promise<string | undefined> {
  const known = await store.get(currentKey, 'arrayBuffer');
  if (known) {
    try {
      const info = validateReleaseInfo(JSON.parse(new TextDecoder().decode(known)));
      // Update checks arrive more often than the release page is read, so
      // follow GitHub at most every five minutes per isolate.
      if (Date.now() - releaseRefreshedAt >= releaseRefresh) {
        releaseRefreshedAt = Date.now();
        context.waitUntil(refreshCurrent(store, fetcher).catch(() => undefined));
      }
      return info.tag;
    } catch {
      /* A damaged stored release is replaced below. */
    }
  }
  const release = await currentRelease(fetcher);
  if (!release) return undefined;
  await saveChanged(store, currentKey, new TextEncoder().encode(JSON.stringify(release)));
  return release.tag;
}
async function fetchAsset(tag: string, name: string, fetcher: typeof fetch, range?: string | null) {
  const requestHeaders: Record<string, string> = { 'User-Agent': 'fujimod-web' };
  if (range) requestHeaders.Range = range;
  return fetcher(releaseAsset(tag, name), { headers: requestHeaders, redirect: 'follow' }).catch(
    () => undefined,
  );
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
/** Relay a range the upstream answered itself; its body holds just that range. */
function partialStoreResponse(
  method: string,
  bytes: Uint8Array<ArrayBuffer>,
  upstream: Response,
): Response {
  const result = storeHeaders(bytes.length);
  const contentRange = upstream.headers.get('Content-Range');
  if (contentRange) result.set('Content-Range', contentRange);
  return new Response(method === 'HEAD' ? null : bytes, { status: 206, headers: result });
}
async function serveManifest(
  method: string,
  firmware: string,
  tag: string,
  fetcher: typeof fetch,
): Promise<Response> {
  const upstream = await fetchAsset(tag, `${firmware}-latest.json`, fetcher);
  if (!upstream?.ok)
    return upstream?.status === 404
      ? failure(404, 'No release manifest has been published for this firmware.')
      : failure(502, 'Release upstream unavailable.');
  const bytes = new Uint8Array(await upstream.arrayBuffer());
  let manifest: unknown;
  try {
    manifest = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return failure(502, 'The release manifest is not valid JSON.');
  }
  // The asset name carries the firmware id; the manifest must agree so a
  // naming mistake never sends one camera another firmware's release.
  if (
    !manifest ||
    typeof manifest !== 'object' ||
    (manifest as { firmware?: unknown }).firmware !== firmware
  )
    return failure(404, 'No release manifest has been published for this firmware.');
  const result = new Headers({
    'Content-Type': 'application/json',
    'Content-Length': String(bytes.length),
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  for (const [name, value] of Object.entries(runtimeHeaders)) result.set(name, value);
  return new Response(method === 'HEAD' ? null : bytes, { headers: result });
}
async function serveStore(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  tag: string,
  name: string,
  fetcher: typeof fetch,
): Promise<Response> {
  const url = new URL(request.url);
  url.search = '';
  const key = new Request(url, { method: 'GET' });
  const range = request.headers.get('Range');
  const cached = await cache.match(key);
  if (cached)
    return storeResponse(request.method, new Uint8Array(await cached.arrayBuffer()), range);
  const upstream = await fetchAsset(tag, name, fetcher, range);
  if (upstream?.status === 416) {
    const result = storeHeaders(0);
    const contentRange = upstream.headers.get('Content-Range');
    if (contentRange) result.set('Content-Range', contentRange);
    result.set('Cache-Control', 'no-store');
    return new Response(null, { status: 416, headers: result });
  }
  if (!upstream?.ok) {
    const known = await store.get(storeKey(name), 'arrayBuffer');
    if (known) return storeResponse(request.method, new Uint8Array(known), range);
    return upstream?.status === 404
      ? failure(404, 'Store file unavailable.')
      : failure(502, 'Release upstream unavailable.');
  }
  const bytes = new Uint8Array(await upstream.arrayBuffer());
  if (upstream.status === 206) return partialStoreResponse(request.method, bytes, upstream);
  const full = new Response(bytes, {
    headers: {
      'Content-Type': 'application/octet-stream',
      'Cache-Control': 'public, max-age=31536000, immutable',
    },
  });
  context.waitUntil(cache.put(key, full));
  context.waitUntil(saveChanged(store, storeKey(name), bytes));
  return storeResponse(request.method, bytes, range);
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
/** Serve `/ota/...` from the current GitHub release. Manifests are fetched on
 * every request; store files are immutable and cached by content name. */
export async function serveOta(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch = fetch,
): Promise<Response> {
  return headOnly(request, await serveOtaPath(request, cache, store, context, fetcher));
}
async function serveOtaPath(
  request: Request,
  cache: ReleaseCache,
  store: ReleaseStore,
  context: Context,
  fetcher: typeof fetch,
): Promise<Response> {
  const path = new URL(request.url).pathname;
  if (path === '/ota/timezone') return serveTimezone(request);
  if (path.startsWith('/ota/')) {
    const route = otaRoute(path);
    if (!route) return failure(404, 'OTA route not found.');
    if (request.method !== 'GET' && request.method !== 'HEAD') return methodNotAllowed();
    try {
      const tag = await currentTag(store, context, fetcher);
      if (!tag) return failure(404, 'No release has been published.');
      return route.kind === 'manifest'
        ? await serveManifest(request.method, route.firmware, tag, fetcher)
        : await serveStore(request, cache, store, context, tag, route.name, fetcher);
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
export default {
  async fetch(
    request: Request,
    env: { ASSETS: Assets; RELEASES: ReleaseStore },
    context: Context,
  ): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path.startsWith('/releases/'))
      return serveRelease(request, await caches.open('fujimod-releases'), env.RELEASES, context);
    if (path.startsWith('/ota/'))
      return serveOta(request, await caches.open('fujimod-ota'), env.RELEASES, context);
    return env.ASSETS.fetch(request);
  },
};
