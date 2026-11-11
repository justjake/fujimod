import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Miniflare } from 'miniflare';
import { afterAll, beforeAll, expect, test } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';

/** End to end through the built Worker: real Better Auth, real D1, real routes. */
const web = fileURLToPath(new URL('../', import.meta.url));
const wrangler = fileURLToPath(
  new URL('../../usb-sharing/node_modules/.bin/wrangler', import.meta.url),
);
const secret = 'e2e-secret-e2e-secret-e2e-secret';
const origin = 'https://fujimod.com';

let miniflare: Miniflare;
let db: D1Database;
let script: string;
let outdir: string;

function bundle(): string {
  const out = mkdtempSync(join(tmpdir(), 'fujimod-e2e-worker-'));
  const config = join(out, 'wrangler.jsonc');
  writeFileSync(
    config,
    JSON.stringify({
      name: 'fujimod-web-e2e',
      main: join(web, 'worker.ts'),
      compatibility_date: '2026-10-17',
      compatibility_flags: ['nodejs_compat'],
    }),
  );
  execFileSync(wrangler, ['deploy', '--dry-run', '--outdir', out, '--config', config], {
    cwd: web,
    stdio: 'pipe',
  });
  outdir = out;
  return join(out, 'worker.js');
}

beforeAll(() => {
  script = readFileSync(bundle(), 'utf8');
});

afterAll(async () => {
  await miniflare?.dispose();
  if (outdir) rmSync(outdir, { recursive: true, force: true });
});

async function runtime(): Promise<D1Database> {
  if (db) return db;
  miniflare = new Miniflare({
    workers: [
      {
        config: {
          name: 'fujimod-web-e2e',
          compatibilityDate: '2026-10-17',
          compatibilityFlags: ['nodejs_compat'],
          manifest: {
            mainModule: 'worker.mjs',
            modules: { 'worker.mjs': { type: 'esm', contents: script } },
          },
          env: {
            AUTH_DB: { type: 'd1', id: 'fujimod-auth' },
            RELEASES: { type: 'kv' },
            BETTER_AUTH_SECRET: { type: 'text', value: secret },
            GOOGLE_CLIENT_ID: { type: 'text', value: 'e2e-client' },
            GOOGLE_CLIENT_SECRET: { type: 'text', value: 'e2e-secret' },
          },
        },
      },
    ],
  });
  const database = (await miniflare.getD1Database('AUTH_DB')) as unknown as D1Database;
  const sql = readFileSync(new URL('../drizzle/0000_married_scourge.sql', import.meta.url), 'utf8');
  for (const statement of sql.split('--> statement-breakpoint'))
    if (statement.trim()) await database.prepare(statement).run();
  await database
    .prepare(
      "insert into user (id, name, email, email_verified, created_at, updated_at) values ('u1', 'Test', 'test@example.com', 0, 1, 1)",
    )
    .run();
  await database
    .prepare(
      "insert into account (id, account_id, provider_id, user_id, access_token, access_token_expires_at, created_at, updated_at) values ('a1', 'google-sub', 'google', 'u1', 'google-token', ?, 1, 1)",
    )
    .bind(Date.now() + 600_000)
    .run();
  db = database;
  return db;
}

function post(path: string, body: unknown, headers: Record<string, string> = {}) {
  return miniflare.dispatchFetch(`${origin}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', origin, ...headers },
    body: JSON.stringify(body),
  });
}

test('a camera signs in through the Worker and receives a Google token', async () => {
  const database = await runtime();

  const issued = await post('/api/auth/device/code', {
    client_id: 'x70-camera',
    scope: 'google.photos.append',
  });
  expect(issued.status).toBe(200);
  const code = (await issued.json()) as {
    device_code: string;
    user_code: string;
    verification_uri_complete: string;
  };
  expect(code.verification_uri_complete).toContain(code.user_code);

  const pending = await miniflare.dispatchFetch(
    `${origin}/api/auth/device?user_code=${encodeURIComponent(code.user_code)}`,
  );
  expect(await pending.json()).toMatchObject({ status: 'pending' });

  // The owner approves in the browser; the Worker only sees the resulting row.
  await database
    .prepare("update device_code set status = 'approved', user_id = 'u1' where device_code = ?")
    .bind(code.device_code)
    .run();

  const redeemed = await post('/api/auth/device/token', {
    grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
    device_code: code.device_code,
    client_id: 'x70-camera',
  });
  expect(redeemed.status).toBe(200);
  const token = (await redeemed.json()) as { access_token: string };
  expect(token.access_token).toBeTruthy();

  const device = await database
    .prepare('select id, name, kind from device')
    .first<{ id: string; name: string; kind: string }>();
  expect(device).toMatchObject({ name: 'Fujifilm camera', kind: 'camera' });

  const google = await miniflare.dispatchFetch(`${origin}/api/devices/google-token`, {
    method: 'POST',
    headers: { authorization: `Bearer ${token.access_token}` },
  });
  expect(google.status).toBe(200);
  expect(await google.json()).toMatchObject({ accessToken: 'google-token' });
}, 120_000);

test('a client the Worker does not know cannot start a device code', async () => {
  await runtime();
  const refused = await post('/api/auth/device/code', {
    client_id: 'someone-else',
    scope: 'google.photos.append',
  });
  expect(refused.status).toBe(400);
}, 120_000);
