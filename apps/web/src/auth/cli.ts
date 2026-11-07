import { drizzleAdapter } from '@better-auth/drizzle-adapter';
import { betterAuth } from 'better-auth/minimal';
import { drizzle } from 'drizzle-orm/d1';
import type { D1Database } from '@cloudflare/workers-types';
import { authOptions } from './options.ts';

// Better Auth's schema generator imports this module. The adapter is not queried.
const db = drizzle({} as D1Database);

export const auth = betterAuth({
  ...authOptions(
    {
      BETTER_AUTH_SECRET: 'schema-generation-only-not-for-production-1234567890',
      GOOGLE_CLIENT_ID: 'schema-generation-only',
      GOOGLE_CLIENT_SECRET: 'schema-generation-only',
    },
    'http://localhost:8787',
  ),
  database: drizzleAdapter(db, { provider: 'sqlite' }),
});
