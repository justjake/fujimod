import type { Env } from './env.ts';
import { firmwareSource } from './firmware-source.ts';

/** User-requested, uncached transfer from an official firmware source. */
export async function firmwareFetch(request: Request, env: Env): Promise<Response> {
  const origin = request.headers.get('Origin') ?? '';
  const allowed = (env.ALLOWED_ORIGINS || '').split(',').map((value) => value.trim());
  const headers = new Headers({ 'Cache-Control': 'no-store', Vary: 'Origin' });
  if (!origin || !allowed.includes(origin))
    return new Response('Origin not allowed', { status: 403, headers });
  headers.set('Access-Control-Allow-Origin', origin);
  const fail = (message: string, status: number) => new Response(message, { status, headers });
  if (request.method !== 'GET') return fail('GET required', 405);
  const target = firmwareSource(new URL(request.url).searchParams.get('url') ?? '');
  if (!target) return fail('Firmware URL not allowed', 400);
  const ip = request.headers.get('CF-Connecting-IP') || 'local';
  if (!(await env.CONNECTIONS.limit({ key: `firmware:${ip}` })).success)
    return fail('Rate exceeded', 429);
  let upstream: Response;
  try {
    upstream = await fetch(target.href, {
      redirect: 'manual',
      cache: 'no-store',
      signal: request.signal,
    });
  } catch {
    return fail('Firmware source unavailable', 502);
  }
  if (upstream.status !== 200) {
    await upstream.body?.cancel();
    return fail('Firmware source did not return a file', 502);
  }
  for (const name of ['Content-Type', 'Content-Length']) {
    const value = upstream.headers.get(name);
    if (value) headers.set(name, value);
  }
  return new Response(upstream.body, { headers });
}
