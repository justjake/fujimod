import type { D1Database } from '@cloudflare/workers-types';
import { devicePolicy } from './policy.ts';
import {
  approvedCode,
  credentialGrant,
  googleAccountId,
  recordCredential,
  touchCredential,
} from './store.ts';

export interface ProviderTokens {
  accessToken?: string | null;
  accessTokenExpiresAt?: Date | string | null;
}

/** The Better Auth call that returns a linked provider token, refreshing it if needed. */
export interface ProviderTokenApi {
  getAccessToken(input: { body: { accountId: string; userId: string } }): Promise<ProviderTokens>;
}

const headers = { 'content-type': 'application/json', 'cache-control': 'no-store' };

function reply(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers });
}

export function bearerToken(header: string | null): string | undefined {
  return header?.match(/^Bearer\s+(\S+)$/i)?.[1];
}

/** Wraps the device token exchange so the issued session is remembered as a device. */
export async function serveDeviceToken(
  request: Request,
  db: D1Database,
  handler: (request: Request) => Promise<Response>,
): Promise<Response> {
  const body = (await request
    .clone()
    .json()
    .catch(() => undefined)) as { device_code?: unknown } | undefined;
  const code = typeof body?.device_code === 'string' ? body.device_code : undefined;
  const approved = code ? await approvedCode(db, code) : undefined;
  const response = await handler(request);
  if (!approved || !response.ok) return response;
  const issued = (await response
    .clone()
    .json()
    .catch(() => undefined)) as { access_token?: unknown } | undefined;
  const policy = devicePolicy(approved.clientId);
  if (typeof issued?.access_token !== 'string' || !policy) return response;
  await recordCredential(db, { userId: approved.userId, token: issued.access_token, policy });
  return response;
}

/** Exchanges a camera's bearer token for the Google token it may upload with. */
export async function serveGoogleToken(
  request: Request,
  db: D1Database,
  auth: ProviderTokenApi,
): Promise<Response> {
  if (request.method !== 'POST') return reply(405, { error: 'method_not_allowed' });
  const token = bearerToken(request.headers.get('authorization'));
  if (!token) return reply(401, { error: 'unauthorized' });
  const grant = await credentialGrant(db, token);
  if (!grant) return reply(401, { error: 'unauthorized' });
  if (!grant.permissions.includes('google.photos.append'))
    return reply(403, { error: 'forbidden' });
  try {
    const accountId = await googleAccountId(db, grant.userId);
    if (!accountId) return reply(409, { error: 'google_account_unavailable' });
    const tokens = await auth.getAccessToken({ body: { accountId, userId: grant.userId } });
    if (!tokens.accessToken) return reply(409, { error: 'google_account_unavailable' });
    await touchCredential(db, token);
    return reply(200, {
      accessToken: tokens.accessToken,
      expiresAt: tokens.accessTokenExpiresAt
        ? new Date(tokens.accessTokenExpiresAt).toISOString()
        : null,
    });
  } catch {
    return reply(409, { error: 'google_account_unavailable' });
  }
}
