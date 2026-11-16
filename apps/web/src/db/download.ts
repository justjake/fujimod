import { index, integer, sqliteTable, text } from 'drizzle-orm/sqlite-core';

export const downloadEvent = sqliteTable(
  'download_event',
  {
    id: integer('id').primaryKey({ autoIncrement: true }),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    release: text('release').notNull(),
    kind: text('kind', { enum: ['installable', 'sd-boot-v1', 'sd-card'] }).notNull(),
    firmware: text('firmware'),
    cameras: integer('cameras').notNull(),
    session: text('session'),
  },
  (table) => [
    index('downloadEvent_createdAt_idx').on(table.createdAt),
    index('downloadEvent_release_idx').on(table.release),
    index('downloadEvent_session_idx').on(table.session),
  ],
);
