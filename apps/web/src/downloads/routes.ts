import type { D1Database } from '@cloudflare/workers-types';
import { parseDownloadEvent } from './event.ts';

const maxBody = 2048;

function reply(status: number, error: string): Response {
  return Response.json({ error }, { status, headers: { 'cache-control': 'no-store' } });
}

/** Record one completed FujiMod download. The client sends this after it has
 * produced the archive, so rejects and failed builds never count. */
export async function serveDownloadEvent(
  request: Request,
  db: D1Database,
  now = new Date(),
): Promise<Response> {
  if (request.method !== 'POST')
    return new Response(null, {
      status: 405,
      headers: { Allow: 'POST', 'cache-control': 'no-store' },
    });
  const type = request.headers.get('content-type')?.split(';', 1)[0].trim().toLowerCase();
  if (type !== 'application/json') return reply(415, 'application/json required');
  const declared = Number(request.headers.get('content-length') ?? 0);
  if (Number.isFinite(declared) && declared > maxBody) return reply(413, 'body too large');
  const text = await request.text();
  if (text.length > maxBody) return reply(413, 'body too large');
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return reply(400, 'invalid JSON');
  }
  const event = parseDownloadEvent(body);
  if (!event) return reply(400, 'invalid download event');
  try {
    await db
      .prepare(
        'insert into download_event (created_at, release, kind, firmware, cameras, session) values (?, ?, ?, ?, ?, ?)',
      )
      .bind(
        now.getTime(),
        event.release,
        event.kind,
        event.firmware ?? null,
        event.cameras,
        event.session ?? null,
      )
      .run();
  } catch {
    return reply(503, 'download counter unavailable');
  }
  return new Response(null, { status: 204, headers: { 'cache-control': 'no-store' } });
}
