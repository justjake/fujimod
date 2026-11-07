import { expect, test, vi } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { serveRequest } from '../worker.ts';
import { authOrigins } from '../src/auth/options.ts';

const env = {
  ASSETS: { fetch: async () => new Response('static') },
  RELEASES: { get: async () => null, put: async () => {} },
  AUTH_DB: {} as D1Database,
  BETTER_AUTH_SECRET: 'test-secret-test-secret-test-secret',
  GOOGLE_CLIENT_ID: 'test-client-id',
  GOOGLE_CLIENT_SECRET: 'test-client-secret',
};

test('the google callback reaches the auth handler', async () => {
  const handler = vi.fn(async () => new Response('callback'));
  const response = await serveRequest(
    new Request('https://fujimod.com/api/auth/callback/google?code=abc&state=def'),
    env,
    { waitUntil() {} },
    () => ({ handler }),
  );

  expect(await response.text()).toBe('callback');
  expect(handler).toHaveBeenCalledOnce();
});

test('www requests sign in through the apex origin', () => {
  expect(authOrigins('https://www.fujimod.com')).toEqual({
    baseURL: 'https://fujimod.com',
    trustedOrigins: ['https://www.fujimod.com', 'https://fujimod.com'],
  });
  expect(authOrigins('https://cozycat.tail2b5ca.ts.net:8794')).toEqual({
    baseURL: 'https://cozycat.tail2b5ca.ts.net:8794',
    trustedOrigins: ['https://cozycat.tail2b5ca.ts.net:8794'],
  });
});

test('auth routes run before static assets', async () => {
  const handler = vi.fn(async () => new Response('auth'));
  const makeAuth = vi.fn(() => ({ handler }));
  const assets = vi.fn(async () => new Response('static'));
  const assetsEnv = { ...env, ASSETS: { fetch: assets } };

  const response = await serveRequest(
    new Request('https://fujimod.com/api/auth/session'),
    assetsEnv,
    { waitUntil() {} },
    makeAuth,
  );

  expect(await response.text()).toBe('auth');
  expect(makeAuth).toHaveBeenCalledWith(assetsEnv, 'https://fujimod.com');
  expect(assets).not.toHaveBeenCalled();
});

test('lookalike auth paths still use static assets', async () => {
  const makeAuth = vi.fn();
  const assets = vi.fn(async () => new Response('static'));

  const response = await serveRequest(
    new Request('https://fujimod.com/api/authish'),
    { ...env, ASSETS: { fetch: assets } },
    { waitUntil() {} },
    makeAuth,
  );

  expect(await response.text()).toBe('static');
  expect(makeAuth).not.toHaveBeenCalled();
});
