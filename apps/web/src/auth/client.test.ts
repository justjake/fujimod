import { afterEach, expect, test, vi } from 'vite-plus/test';
import { deviceRequest, googleSignInUrl, resolveDevice, sessionUser } from './client.ts';

interface Call {
  url: string;
  init?: RequestInit;
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === 'string') return input;
  return input instanceof URL ? input.href : input.url;
}

function sentBody(call: Call): unknown {
  return JSON.parse(call.init?.body as string);
}

function stub(handler: (call: Call) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const call = { url: requestUrl(input), init };
    calls.push(call);
    return handler(call);
  });
  return calls;
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

afterEach(() => vi.unstubAllGlobals());

test('reads the signed-in user', async () => {
  const calls = stub(() => json({ user: { id: 'u1', email: 'a@b.c', name: 'A' }, session: {} }));
  expect(await sessionUser()).toEqual({ id: 'u1', email: 'a@b.c', name: 'A' });
  expect(calls[0].url).toBe('/api/auth/get-session');
  expect(calls[0].init?.credentials).toBe('same-origin');
});

test('a signed-out visitor has no session', async () => {
  stub(() => json(null));
  expect(await sessionUser()).toBeNull();
});

test('a page served without auth routes counts as signed out', async () => {
  stub(() => new Response('<!doctype html><title>FujiMod</title>', { status: 200 }));
  expect(await sessionUser()).toBeNull();
});

test('sign-in asks Better Auth for the Google URL', async () => {
  const calls = stub(() => json({ url: 'https://accounts.google.com/x', redirect: true }));
  expect(await googleSignInUrl('/device/?user_code=ABC')).toBe('https://accounts.google.com/x');
  expect(calls[0].url).toBe('/api/auth/sign-in/social');
  expect(sentBody(calls[0])).toEqual({
    provider: 'google',
    callbackURL: '/device/?user_code=ABC',
  });
});

test('a device code travels in the query string', async () => {
  const calls = stub(() => json({ user_code: 'AB 1', status: 'pending', client_id: 'camera' }));
  expect((await deviceRequest('AB 1')).status).toBe('pending');
  expect(calls[0].url).toBe('/api/auth/device?user_code=AB%201');
});

test('approval and denial post the user code', async () => {
  const calls = stub(() => json({ success: true }));
  await resolveDevice('2GK5QTNV', true);
  await resolveDevice('2GK5QTNV', false);
  expect(calls.map((call) => call.url)).toEqual([
    '/api/auth/device/approve',
    '/api/auth/device/deny',
  ]);
  expect(calls.map(sentBody)).toEqual([{ userCode: '2GK5QTNV' }, { userCode: '2GK5QTNV' }]);
});

test('the server explanation survives a rejected code', async () => {
  stub(() => json({ error: 'invalid_request', error_description: 'Invalid user code' }, 400));
  await expect(deviceRequest('NOPE')).rejects.toThrow('Invalid user code');
});
