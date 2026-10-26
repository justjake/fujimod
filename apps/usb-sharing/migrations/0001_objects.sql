-- One row per object in the private R2 bucket, written by bucket.ts with every store.
CREATE TABLE objects (
  key TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  id TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  content_type TEXT,
  summary TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  stored_at TEXT NOT NULL
);
CREATE INDEX objects_by_item ON objects (kind, id);
CREATE INDEX objects_by_time ON objects (stored_at);
