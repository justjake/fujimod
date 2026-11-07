import { runtimeHeaders } from './src/security.ts';
import { releaseRoute, type ReleaseInfo } from './src/patch/releases.ts';
import { createAuth } from './src/auth/runtime.ts';
import type { AuthEnv } from './src/auth/options.ts';

const repository = 'justjake/fujimod';
const api = `https://api.github.com/repos/${repository}/releases`;
const headers = { Accept: 'application/vnd.github+json', 'User-Agent': 'fujimod-web' };
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
}
interface AuthHandler {
  handler(request: Request): Promise<Response>;
}
type AuthFactory = (env: AuthEnv, baseURL: string) => AuthHandler;
const currentKey = 'current';
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
function equalBytes(a: Uint8Array, b: Uint8Array) {
  return a.length === b.length && a.every((byte, index) => byte === b[index]);
}
function failure(status: number, message: string) {
  return new Response(message, {
    status,
    headers: { ...runtimeHeaders, 'Cache-Control': 'no-store' },
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
  if (request.method !== 'GET' && request.method !== 'HEAD')
    return new Response(null, {
      status: 405,
      headers: { Allow: 'GET, HEAD', 'Cache-Control': 'no-store' },
    });
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
      const refresh = async () => {
        const release = await currentRelease(fetcher);
        if (!release) return;
        const text = new TextEncoder().encode(JSON.stringify(release));
        await saveChanged(store, currentKey, text);
        return text;
      };
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
      const upstream = await fetcher(
        `https://github.com/${repository}/releases/download/${encodeURIComponent(route.tag)}/${encodeURIComponent(route.file)}`,
        { headers: { 'User-Agent': 'fujimod-web' }, redirect: 'follow' },
      ).catch(() => undefined);
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
export async function serveRequest(
  request: Request,
  env: Env,
  context: Context,
  makeAuth: AuthFactory = createAuth,
): Promise<Response> {
  const url = new URL(request.url);
  if (url.pathname === '/api/auth' || url.pathname.startsWith('/api/auth/'))
    return makeAuth(env, url.origin).handler(request);
  if (!url.pathname.startsWith('/releases/')) return env.ASSETS.fetch(request);
  return serveRelease(request, await caches.open('fujimod-releases'), env.RELEASES, context);
}

export default { fetch: serveRequest };
