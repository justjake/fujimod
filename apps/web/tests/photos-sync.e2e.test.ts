import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Miniflare } from 'miniflare';
import { afterAll, beforeAll, expect, test } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { syncCameraPhotos } from '../src/sync/plugin.ts';
import { startFakePhotos, type FakePhotos } from './support/fake-photos.ts';
import { emulatorAvailable, encodePng, renderEmulatorPhoto } from './support/emulator-photo.ts';

/** End to end: a camera signs in through the real Worker, receives the owner's
 * Google token, and uploads photo bytes to a Google Photos stand-in. */
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
  const out = mkdtempSync(join(tmpdir(), 'fujimod-sync-e2e-worker-'));
  const config = join(out, 'wrangler.jsonc');
  writeFileSync(
    config,
    JSON.stringify({
      name: 'fujimod-sync-e2e',
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
          name: 'fujimod-sync-e2e',
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

/** Sends Worker requests to Miniflare and everything else to the fake backend. */
function transport(): typeof fetch {
  return (async (input: string | URL | Request, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (url.origin === origin)
      return miniflare.dispatchFetch(
        url.href,
        init as Parameters<Miniflare['dispatchFetch']>[1],
      ) as unknown as Promise<Response>;
    return fetch(url, init);
  }) as typeof fetch;
}

async function approve(database: D1Database, deviceCode: string): Promise<void> {
  await database
    .prepare("update device_code set status = 'approved', user_id = 'u1' where device_code = ?")
    .bind(deviceCode)
    .run();
}

const sha256 = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');

test('a camera uploads a photo to Google Photos through the Worker', async () => {
  const database = await runtime();
  const photos: FakePhotos = await startFakePhotos();
  try {
    const png = encodePng(
      2,
      2,
      new Uint8Array([0, 4, 86, 255, 0, 4, 86, 255, 0, 4, 86, 255, 0, 4, 86, 255]),
    );
    let shown: string | undefined;

    const report = await syncCameraPhotos({
      serverUrl: origin,
      photosBaseUrl: photos.baseUrl,
      fetch: transport(),
      source: {
        photos: () => [{ fileName: 'DSCF0001.PNG', bytes: png, contentType: 'image/png' }],
      },
      onUserCode: async (code) => {
        shown = code.userCode;
        await approve(database, code.deviceCode);
      },
    });

    expect(shown).toBeTruthy();
    expect(report.uploaded).toEqual([{ fileName: 'DSCF0001.PNG', mediaItemId: 'media-item-1' }]);
    expect(photos.uploads).toHaveLength(1);
    expect(sha256(photos.uploads[0]!)).toBe(sha256(png));
    expect(photos.created).toEqual([
      { fileName: 'DSCF0001.PNG', uploadToken: 'upload-1', authorization: 'Bearer google-token' },
    ]);
    const device = await database
      .prepare("select name, kind from device where kind = 'camera'")
      .first<{ name: string; kind: string }>();
    expect(device).toMatchObject({ name: 'Fujifilm camera', kind: 'camera' });
  } finally {
    await photos.close();
  }
}, 120_000);

test.skipIf(!emulatorAvailable())(
  'an emulator frame reaches Google Photos byte for byte',
  async () => {
    const database = await runtime();
    const photos = await startFakePhotos();
    try {
      const frame = await renderEmulatorPhoto();
      const report = await syncCameraPhotos({
        serverUrl: origin,
        photosBaseUrl: photos.baseUrl,
        fetch: transport(),
        source: {
          photos: () => [{ fileName: 'DSCF0002.PNG', bytes: frame.png, contentType: 'image/png' }],
        },
        onUserCode: async (code) => approve(database, code.deviceCode),
      });

      expect(report.uploaded).toEqual([{ fileName: 'DSCF0002.PNG', mediaItemId: 'media-item-1' }]);
      expect(photos.uploads).toHaveLength(1);
      expect(sha256(photos.uploads[0]!)).toBe(sha256(frame.png));
      expect(frame.png.length).toBeGreaterThan(50);
    } finally {
      await photos.close();
    }
  },
  120_000,
);
