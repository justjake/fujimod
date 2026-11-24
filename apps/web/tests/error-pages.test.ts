import { afterEach, beforeEach, expect, test, vi } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { serveRequest } from '../worker.ts';

const pages = vi.fn(
  async (request: Request) =>
    new Response(`page ${new URL(request.url).pathname}`, {
      headers: { 'Content-Type': 'text/html' },
    }),
);
function serve(path: string, navigate: boolean, makeAuth?: Parameters<typeof serveRequest>[3]) {
  return serveRequest(
    new Request(`https://fujimod.com${path}`, {
      headers: navigate ? { 'Sec-Fetch-Mode': 'navigate' } : {},
    }),
    {
      ASSETS: { fetch: pages },
      RELEASES: { get: async () => null, put: async () => {} },
      AUTH_DB: {} as D1Database,
      BETTER_AUTH_SECRET: 'test-secret-test-secret-test-secret',
      GOOGLE_CLIENT_ID: 'test-client-id',
      GOOGLE_CLIENT_SECRET: 'test-client-secret',
    },
    { waitUntil() {} },
    makeAuth,
  );
}
beforeEach(() => {
  vi.stubGlobal('caches', {
    open: async () => ({ match: async () => undefined, put: async () => {} }),
  });
});
afterEach(() => {
  pages.mockClear();
  vi.unstubAllGlobals();
});

test('a browser opening a missing release address gets the Page not found page', async () => {
  const response = await serve('/releases/no/such/path', true);
  expect(response.status).toBe(404);
  expect(response.headers.get('Cache-Control')).toBe('no-store');
  expect(await response.text()).toBe('page /404');
});

test('other clients keep the plain failure', async () => {
  const response = await serve('/releases/no/such/path', false);
  expect(response.status).toBe(404);
  expect(await response.text()).toBe('Release route not found.');
  expect(pages).not.toHaveBeenCalled();
});

test('a Worker crash shows the Something went wrong page', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  const crash = () => {
    throw Error('auth exploded');
  };
  const response = await serve('/api/auth/callback/google', true, crash);
  expect(response.status).toBe(500);
  expect(await response.text()).toBe('page /500');
  const plain = await serve('/api/auth/callback/google', false, crash);
  expect(plain.status).toBe(500);
  expect(await plain.text()).toBe('Internal error.');
});

test('a GitHub outage shows the Temporarily unavailable page', async () => {
  vi.stubGlobal('fetch', async () => new Response(null, { status: 503 }));
  const response = await serve('/releases/v0.2.0/patches.json', true);
  expect(response.status).toBe(502);
  expect(await response.text()).toBe('page /503');
});
