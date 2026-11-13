import { expect, test } from 'vite-plus/test';
import type { D1Database } from '@cloudflare/workers-types';
import { parseDownloadEvent } from './event.ts';
import { serveDownloadEvent } from './routes.ts';

const firmware = 'a'.repeat(64);
const session = '123e4567-e89b-42d3-a456-426614174000';

function database(fails = false) {
  const statements: string[] = [];
  const binds: unknown[][] = [];
  const prepare = (sql: string) => {
    const statement = {
      bind: (...values: unknown[]) => {
        binds.push(values);
        return statement;
      },
      async run() {
        statements.push(sql);
        if (fails) throw Error('database unavailable');
        return { success: true };
      },
    };
    return statement;
  };
  return { db: { prepare } as unknown as D1Database, statements, binds };
}

function request(body: unknown, method = 'POST', type = 'application/json') {
  const init: RequestInit = { method, headers: { 'content-type': type } };
  if (method === 'POST') init.body = JSON.stringify(body);
  return new Request('https://fujimod.com/api/downloads', {
    ...init,
  });
}

test('a completed single-camera download is recorded', async () => {
  const store = database();
  const when = new Date('2026-11-13T12:00:00Z');
  const response = await serveDownloadEvent(
    request({ release: '0.0.1', kind: 'installable', firmware, cameras: 1, session }),
    store.db,
    when,
  );

  expect(response.status).toBe(204);
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(store.statements).toEqual([
    'insert into download_event (created_at, release, kind, firmware, cameras, session) values (?, ?, ?, ?, ?, ?)',
  ]);
  expect(store.binds).toEqual([[when.getTime(), '0.0.1', 'installable', firmware, 1, session]]);
});

test('an SD card download records its camera count without a firmware hash', async () => {
  const store = database();
  const response = await serveDownloadEvent(
    request({ release: '0.0.1', kind: 'sd-card', cameras: 3 }),
    store.db,
  );

  expect(response.status).toBe(204);
  expect(store.binds[0]?.slice(1)).toEqual(['0.0.1', 'sd-card', null, 3, null]);
});

test('malformed events and unsupported methods are refused', async () => {
  const store = database();
  expect((await serveDownloadEvent(request({}, 'GET'), store.db)).status).toBe(405);
  expect((await serveDownloadEvent(request({}, 'POST', 'text/plain'), store.db)).status).toBe(415);
  expect((await serveDownloadEvent(request({ release: '0.0.1' }), store.db)).status).toBe(400);
  expect(
    (
      await serveDownloadEvent(
        request({ release: '0.0.1', kind: 'installable', firmware: 'short', cameras: 1 }),
        store.db,
      )
    ).status,
  ).toBe(400);
  expect(store.statements).toEqual([]);
});

test('an unavailable database does not fail the download', async () => {
  const response = await serveDownloadEvent(
    request({ release: '0.0.1', kind: 'installable', firmware, cameras: 1 }),
    database(true).db,
  );
  expect(response.status).toBe(503);
});

test('download events require bounded, well-formed fields', () => {
  expect(
    parseDownloadEvent({ release: '0.0.1', kind: 'installable', firmware, cameras: 1 }),
  ).toEqual({ release: '0.0.1', kind: 'installable', firmware, cameras: 1 });
  for (const value of [
    null,
    [],
    { release: '', kind: 'installable', firmware, cameras: 1 },
    { release: '0.0.1', kind: 'other', firmware, cameras: 1 },
    { release: '0.0.1', kind: 'installable', cameras: 1 },
    { release: '0.0.1', kind: 'sd-card', firmware, cameras: 1 },
    { release: '0.0.1', kind: 'sd-card', cameras: 0 },
    { release: '0.0.1', kind: 'sd-card', cameras: 257 },
    { release: '0.0.1', kind: 'sd-card', cameras: 1, session: 'not-a-session' },
  ])
    expect(parseDownloadEvent(value)).toBeUndefined();
});
