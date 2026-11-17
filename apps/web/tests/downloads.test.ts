import { expect, test, vi } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { serveRequest } from '../worker.ts';

test('download events run before the static asset binding', async () => {
  const statements: string[] = [];
  const database = {
    prepare(sql: string) {
      const statement = {
        bind: () => statement,
        async run() {
          statements.push(sql);
          return { success: true };
        },
      };
      return statement;
    },
  } as unknown as D1Database;
  const assets = vi.fn(async () => new Response('static'));
  const response = await serveRequest(
    new Request('https://fujimod.com/api/downloads', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        release: '0.0.1',
        kind: 'sd-card',
        cameras: 2,
      }),
    }),
    {
      ASSETS: { fetch: assets },
      RELEASES: { get: async () => null, put: async () => {} },
      AUTH_DB: database,
      BETTER_AUTH_SECRET: 'test-secret-test-secret-test-secret',
      GOOGLE_CLIENT_ID: 'test-client-id',
      GOOGLE_CLIENT_SECRET: 'test-client-secret',
    },
    { waitUntil() {} },
  );

  expect(response.status).toBe(204);
  expect(assets).not.toHaveBeenCalled();
  expect(statements).toHaveLength(1);
  expect(statements[0]).toContain('insert into download_event');
});
