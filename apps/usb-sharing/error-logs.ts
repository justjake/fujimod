import type { Env } from './env.ts';
import { clientErrorLog, parseClientError, type ClientErrorRecord } from './client-error-log.ts';
const MAX_RECORD = 16384;
const headers = { 'Cache-Control': 'no-store' };
const json = (value: object, status = 200) => Response.json(value, { status, headers });
/** Browser error intake for the website. Records go to Workers Logs only. */
export async function errorLogs(request: Request, env: Env) {
  const origin = request.headers.get('Origin') ?? '';
  const allowed = (env.ALLOWED_ORIGINS || '').split(',').map((s) => s.trim());
  if (!allowed.includes(origin)) return json({ error: 'Origin not allowed' }, 403);
  const response = await intake(request, env);
  const cors = new Headers(response.headers);
  cors.set('Access-Control-Allow-Origin', origin);
  cors.set('Vary', 'Origin');
  return new Response(response.body, { status: response.status, headers: cors });
}

async function intake(request: Request, env: Env) {
  if (request.method !== 'POST') return json({ error: 'POST required' }, 405);
  if (!env.ERROR_LIMIT) return json({ error: 'Not configured' }, 503);
  const ip = request.headers.get('CF-Connecting-IP') || 'local';
  if (!(await env.ERROR_LIMIT.limit({ key: ip })).success)
    return json({ error: 'Rate exceeded' }, 429);
  let record: ClientErrorRecord;
  try {
    const text = await bounded(request);
    if (text === undefined) throw Error();
    record = parseClientError(text);
  } catch {
    return json({ error: 'Invalid record' }, 400);
  }
  const country = (request as Request & { cf?: { country?: string } }).cf?.country;
  console.log(clientErrorLog(record, country));
  return json({ id: record.id }, 201);
}

async function bounded(request: Request) {
  if (!request.body) return undefined;
  const reader = request.body.getReader(),
    decoder = new TextDecoder();
  let total = 0,
    text = '';
  for (;;) {
    const part = await reader.read();
    if (part.done) return text + decoder.decode();
    total += part.value.length;
    if (total > MAX_RECORD) {
      await reader.cancel();
      return undefined;
    }
    text += decoder.decode(part.value, { stream: true });
  }
}
