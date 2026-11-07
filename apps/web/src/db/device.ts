import { index, integer, primaryKey, sqliteTable, text } from 'drizzle-orm/sqlite-core';
import { user } from './auth-schema.generated.ts';

export const device = sqliteTable(
  'device',
  {
    id: text('id').primaryKey(),
    userId: text('user_id')
      .notNull()
      .references(() => user.id, { onDelete: 'cascade' }),
    name: text('name').notNull(),
    kind: text('kind', { enum: ['camera', 'sync_server'] }).notNull(),
    status: text('status', { enum: ['active', 'revoked'] }).default('active').notNull(),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    lastSeenAt: integer('last_seen_at', { mode: 'timestamp_ms' }),
    revokedAt: integer('revoked_at', { mode: 'timestamp_ms' }),
  },
  (table) => [index('device_userId_idx').on(table.userId)],
);

export const deviceCredential = sqliteTable(
  'device_credential',
  {
    id: text('id').primaryKey(),
    deviceId: text('device_id')
      .notNull()
      .references(() => device.id, { onDelete: 'cascade' }),
    tokenHash: text('token_hash').notNull().unique(),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    expiresAt: integer('expires_at', { mode: 'timestamp_ms' }),
    lastUsedAt: integer('last_used_at', { mode: 'timestamp_ms' }),
    revokedAt: integer('revoked_at', { mode: 'timestamp_ms' }),
  },
  (table) => [index('deviceCredential_deviceId_idx').on(table.deviceId)],
);

export const devicePermission = sqliteTable(
  'device_permission',
  {
    deviceId: text('device_id')
      .notNull()
      .references(() => device.id, { onDelete: 'cascade' }),
    permission: text('permission', {
      enum: ['google.photos.append', 'sync.read', 'sync.write'],
    }).notNull(),
  },
  (table) => [primaryKey({ columns: [table.deviceId, table.permission] })],
);
