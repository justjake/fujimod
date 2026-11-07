import type { BetterAuthOptions } from 'better-auth';
import { deviceAuthorization } from 'better-auth/plugins';
import type { D1Database } from '@cloudflare/workers-types';
import { isDeviceClient } from '../devices/policy.ts';

export interface AuthEnv {
  AUTH_DB: D1Database;
  BETTER_AUTH_SECRET: string;
  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
}

/** Photos scopes the web login adds to Better Auth's default `openid email
 * profile`. The stored account token can then upload straight to the user's
 * library, and devices only receive a copy when their permissions allow it. */
const googleScopes = [
  'https://www.googleapis.com/auth/photoslibrary.appendonly',
  'https://www.googleapis.com/auth/photoslibrary.readonly.appcreateddata',
  'https://www.googleapis.com/auth/photospicker.mediaitems.readonly',
];

/** Better Auth derives the Google redirect URI from the request origin. Requests
 * for `www` resolve to the apex site so one registered redirect covers both, and
 * both origins may call the API. */
export function authOrigins(requestOrigin: string): {
  baseURL: string;
  trustedOrigins: string[];
} {
  const request = new URL(requestOrigin);
  const apex = new URL(requestOrigin);
  if (apex.hostname.startsWith('www.')) apex.hostname = apex.hostname.slice(4);
  return {
    baseURL: apex.origin,
    trustedOrigins: [...new Set([request.origin, apex.origin])],
  };
}

/** Shared Better Auth policy for the Worker and the schema-generation CLI. */
export function authOptions(
  env: Pick<AuthEnv, 'BETTER_AUTH_SECRET' | 'GOOGLE_CLIENT_ID' | 'GOOGLE_CLIENT_SECRET'>,
  requestOrigin: string,
): BetterAuthOptions {
  const { baseURL, trustedOrigins } = authOrigins(requestOrigin);
  return {
    baseURL,
    secret: env.BETTER_AUTH_SECRET,
    account: { encryptOAuthTokens: true },
    trustedOrigins,
    emailAndPassword: { enabled: false },
    plugins: [deviceAuthorization({ validateClient: isDeviceClient })],
    socialProviders: {
      google: {
        clientId: env.GOOGLE_CLIENT_ID,
        clientSecret: env.GOOGLE_CLIENT_SECRET,
        scope: googleScopes,
        accessType: 'offline',
        prompt: 'select_account',
      },
    },
  };
}
