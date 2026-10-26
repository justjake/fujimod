-- A row is written when an upload starts and marked stored when it finishes.
CREATE TABLE objects_v2 (
  key TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('uploading', 'stored')),
  bytes INTEGER,
  content_type TEXT,
  summary TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  started_at TEXT NOT NULL,
  stored_at TEXT
);
INSERT INTO objects_v2 (key, kind, id, state, bytes, content_type, summary, metadata, started_at, stored_at)
  SELECT key, kind, id, 'stored', bytes, content_type, summary, metadata, stored_at, stored_at FROM objects;
DROP TABLE objects;
ALTER TABLE objects_v2 RENAME TO objects;
CREATE INDEX objects_by_item ON objects (kind, id);
CREATE INDEX objects_by_time ON objects (started_at);
CREATE INDEX objects_by_state ON objects (state);
