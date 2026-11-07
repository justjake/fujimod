import { readFileSync } from 'node:fs';
import { Miniflare } from 'miniflare';
import { afterAll, expect, test } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { serveDeviceToken, serveGoogleToken } from '../src/devices/routes.ts';

/** These cases run the device SQL against a real D1 database inside workerd. */
const miniflare = new Miniflare({
  workers: [
    {
      config: {
        name: 'auth',
        compatibilityDate: '2026-10-17',
        manifest: {
          mainModule: 'worker.mjs',
          modules: {
            'worker.mjs': { type: 'esm', contents: 'export default { fetch: () => new Response("ok") }' },
          },
        },
        env: { AUTH_DB: { type: 'd1', id: 'fujimod-auth' } },
      },
    },
  ],
});

let migrated: Promise<D1Database> | undefined;

function database(): Promise<D1Database> {
  migrated ??= (async () => {
    const db = (await miniflare.getD1Database('AUTH_DB')) as unknown as D1Database;
    const sql = readFileSync(new URL('../drizzle/0000_married_scourge.sql', import.meta.url), 'utf8');
    for (const statement of sql.split('--> statement-breakpoint'))
      if (statement.trim()) await db.prepare(statement).run();
    await db.batch([
      db.prepare(
        "insert into user (id, name, email, email_verified, created_at, updated_at) values ('u1', 'Test', 'test@example.com', 0, 1, 1)",
      ),
      db.prepare(
        "insert into account (id, account_id, provider_id, user_id, access_token, access_token_expires_at, created_at, updated_at) values ('a1', 'google-sub', 'google', 'u1', 'google-token', ?, 1, 1)",
      ).bind(Date.now() + 600_000),
    ]);
    return db;
  })();
  return migrated;
}

async function approvedCode(code: string) {
  const db = await database();
  await db
    .prepare(
      "insert into device_code (id, device_code, user_code, user_id, expires_at, status, polling_interval, client_id, scope) values (?, ?, ?, 'u1', ?, 'approved', 5, 'x70-camera', 'google.photos.append')",
    )
    .bind(`id-${code}`, code, code.toUpperCase(), Date.now() + 600_000)
    .run();
}

async function redeem(code: string): Promise<string> {
  const db = await database();
  const response = await serveDeviceToken(
    new Request('https://fujimod.com/api/auth/device/token', {
      method: 'POST',
      body: JSON.stringify({ device_code: code, client_id: 'x70-camera' }),
    }),
    db,
    async () => new Response(JSON.stringify({ access_token: 'camera-token' }), { status: 200 }),
  );
  expect(response.status).toBe(200);
  return 'camera-token';
}

function broker(token: string) {
  return new Request('https://fujimod.com/api/devices/google-token', {
    method: 'POST',
    headers: { authorization: `Bearer ${token}` },
  });
}

const tokens = {
  getAccessToken: async () => ({
    accessToken: 'google-access-token',
    accessTokenExpiresAt: new Date(Date.now() + 3_600_000),
  }),
};

afterAll(() => miniflare.dispose());

test(
  'a redeemed device code becomes a permitted camera with a usable credential',
  async () => {
    const db = await database();
    await approvedCode('code-one');
    const token = await redeem('code-one');

    const device = await db
      .prepare('select id, name, kind, user_id as userId from device')
      .first<{ id: string; name: string; kind: string; userId: string }>();
    expect(device).toMatchObject({ name: 'Fujifilm camera', kind: 'camera', userId: 'u1' });
    const permissions = await db
      .prepare('select permission from device_permission where device_id = ?')
      .bind(device!.id)
      .all<{ permission: string }>();
    expect(permissions.results.map((row) => row.permission)).toEqual(['google.photos.append']);

    const response = await serveGoogleToken(broker(token), db, tokens);
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ accessToken: 'google-access-token' });

    const used = await db
      .prepare('select last_used_at as usedAt from device_credential where token_hash is not null')
      .first<{ usedAt: number | null }>();
    expect(used?.usedAt).toBeGreaterThan(0);
  },
  30_000,
);

test(
  'a credential without the photos permission is refused',
  async () => {
    const db = await database();
    await approvedCode('code-two');
    const token = await redeem('code-two');
    await db.prepare("delete from device_permission where permission = 'google.photos.append'").run();

    expect((await serveGoogleToken(broker(token), db, tokens)).status).toBe(403);
  },
  30_000,
);

test(
  'a revoked device loses access to the Google token',
  async () => {
    const db = await database();
    await approvedCode('code-three');
    const token = await redeem('code-three');
    await db.prepare("update device set status = 'revoked', revoked_at = 1").run();

    expect((await serveGoogleToken(broker(token), db, tokens)).status).toBe(401);
  },
  30_000,
);

test('an unknown token is refused', async () => {
  const db = await database();
  expect((await serveGoogleToken(broker('made-up'), db, tokens)).status).toBe(401);
}, 30_000);
