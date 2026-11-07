import { expect, test, vi } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { bearerToken, serveDeviceToken, serveGoogleToken } from './routes.ts';

interface Rows {
  approved?: { userId: string; clientId: string };
  grant?: { deviceId: string; userId: string };
  permissions?: string[];
  accountId?: string;
}

/** The handful of statements these routes issue, answered from a case. */
function database(rows: Rows, statements: string[] = []): D1Database {
  const prepare = (sql: string) => {
    const shape = sql.replace(/\s+/g, ' ').trim();
    const statement = {
      bind: () => statement,
      async first() {
        statements.push(shape);
        if (shape.includes('from device_code')) return rows.approved ?? null;
        if (shape.includes('join device on')) return rows.grant ?? null;
        if (shape.includes('from account')) return rows.accountId ? { id: rows.accountId } : null;
        return null;
      },
      async all() {
        statements.push(shape);
        return { results: (rows.permissions ?? []).map((permission) => ({ permission })) };
      },
      async run() {
        statements.push(shape);
        return { success: true };
      },
    };
    return statement;
  };
  return {
    prepare,
    async batch(batch: { run(): Promise<unknown> }[]) {
      for (const entry of batch) await entry.run();
      return [];
    },
  } as unknown as D1Database;
}

function tokens(accessToken: string | null = 'google-token') {
  return { getAccessToken: vi.fn(async () => ({ accessToken })) };
}

function request(headers: Record<string, string>, method = 'POST') {
  return new Request('https://fujimod.com/api/devices/google-token', { method, headers });
}

test('bearer tokens are read from the authorization header', () => {
  expect(bearerToken('Bearer abc123')).toBe('abc123');
  expect(bearerToken('bearer abc123')).toBe('abc123');
  expect(bearerToken('Basic abc123')).toBeUndefined();
  expect(bearerToken(null)).toBeUndefined();
});

test('only a POST with a device token is answered', async () => {
  expect((await serveGoogleToken(request({}, 'GET'), database({}), tokens())).status).toBe(405);
  expect((await serveGoogleToken(request({}), database({}), tokens())).status).toBe(401);
});

test('a device without the photos permission is refused', async () => {
  const api = tokens();
  const db = database({ grant: { deviceId: 'd1', userId: 'u1' }, permissions: ['sync.read'] });
  const response = await serveGoogleToken(request({ authorization: 'Bearer camera' }), db, api);
  expect(response.status).toBe(403);
  expect(api.getAccessToken).not.toHaveBeenCalled();
});

test('a camera without a linked Google account is told to link one', async () => {
  const db = database({
    grant: { deviceId: 'd1', userId: 'u1' },
    permissions: ['google.photos.append'],
  });
  const response = await serveGoogleToken(
    request({ authorization: 'Bearer camera' }),
    db,
    tokens(),
  );
  expect(response.status).toBe(409);
  expect(await response.json()).toEqual({ error: 'google_account_unavailable' });
});

test('a permitted camera receives the user Google token', async () => {
  const statements: string[] = [];
  const api = tokens();
  const db = database(
    {
      grant: { deviceId: 'd1', userId: 'u1' },
      permissions: ['google.photos.append'],
      accountId: 'account-1',
    },
    statements,
  );
  const response = await serveGoogleToken(request({ authorization: 'Bearer camera' }), db, api);
  expect(response.status).toBe(200);
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(await response.json()).toEqual({ accessToken: 'google-token', expiresAt: null });
  expect(api.getAccessToken).toHaveBeenCalledWith({
    body: { accountId: 'account-1', userId: 'u1' },
  });
  expect(statements.some((sql) => sql.startsWith('update device_credential'))).toBe(true);
});

test('a redeemed code is remembered as the camera device', async () => {
  const statements: string[] = [];
  const db = database({ approved: { userId: 'u1', clientId: 'x70-camera' } }, statements);
  const handler = vi.fn(
    async () => new Response(JSON.stringify({ access_token: 'camera-token' }), { status: 200 }),
  );
  const response = await serveDeviceToken(
    new Request('https://fujimod.com/api/auth/device/token', {
      method: 'POST',
      body: JSON.stringify({ device_code: 'code-1', client_id: 'x70-camera' }),
    }),
    db,
    handler,
  );
  expect(await response.json()).toEqual({ access_token: 'camera-token' });
  expect(statements.some((sql) => sql.startsWith('insert into device ('))).toBe(true);
  expect(statements.some((sql) => sql.startsWith('insert or ignore into device_permission'))).toBe(
    true,
  );
  expect(statements.some((sql) => sql.startsWith('insert or ignore into device_credential'))).toBe(
    true,
  );
});

test('a failed exchange records nothing', async () => {
  const statements: string[] = [];
  const db = database({ approved: { userId: 'u1', clientId: 'x70-camera' } }, statements);
  const response = await serveDeviceToken(
    new Request('https://fujimod.com/api/auth/device/token', {
      method: 'POST',
      body: JSON.stringify({ device_code: 'code-1' }),
    }),
    db,
    async () => new Response('pending', { status: 400 }),
  );
  expect(response.status).toBe(400);
  expect(statements.filter((sql) => sql.startsWith('insert'))).toEqual([]);
});

test('a form-encoded token request is still recorded', async () => {
  const statements: string[] = [];
  const db = database({ approved: { userId: 'u1', clientId: 'x70-camera' } }, statements);
  const response = await serveDeviceToken(
    new Request('https://fujimod.com/api/auth/device/token', {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({
        grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
        device_code: 'code-form',
        client_id: 'x70-camera',
      }).toString(),
    }),
    db,
    async () => new Response(JSON.stringify({ access_token: 'form-token' }), { status: 200 }),
  );
  expect(response.status).toBe(200);
  expect(statements.some((sql) => sql.startsWith('insert or ignore into device_credential'))).toBe(
    true,
  );
});
