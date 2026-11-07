import { drizzleAdapter } from '@better-auth/drizzle-adapter';
import { betterAuth } from 'better-auth/minimal';
import { drizzle } from 'drizzle-orm/d1';
import * as schema from '../db/schema.ts';
import { authOptions, type AuthEnv } from './options.ts';

export function createAuth(env: AuthEnv, baseURL: string) {
  const db = drizzle(env.AUTH_DB, { schema });
  return betterAuth({
    ...authOptions(env, baseURL),
    database: drizzleAdapter(db, { provider: 'sqlite', schema }),
  });
}
