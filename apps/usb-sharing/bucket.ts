import type { Env } from './env.ts';

/** What an object in the private bucket belongs to. Every write records it in the D1 index. */
export interface IndexEntry {
  kind: 'issue-report' | 'contribution';
  id: string;
  /** One line a maintainer reads in a listing. */
  summary?: string;
  metadata?: Record<string, unknown>;
}

/** Writes the row before any bytes go to R2, so abandoned uploads stay visible. */
async function begin(env: Env, key: string, entry: IndexEntry, contentType?: string) {
  await env.REPORTS_INDEX.prepare(
    `INSERT INTO objects (key, kind, id, state, content_type, summary, metadata, started_at)
     VALUES (?1, ?2, ?3, 'uploading', ?4, ?5, ?6, ?7)
     ON CONFLICT (key) DO UPDATE SET kind = ?2, id = ?3, state = 'uploading', bytes = NULL,
       content_type = ?4, summary = ?5, metadata = ?6, started_at = ?7, stored_at = NULL`,
  )
    .bind(
      key,
      entry.kind,
      entry.id,
      contentType ?? null,
      entry.summary ?? null,
      JSON.stringify(entry.metadata ?? {}),
      new Date().toISOString(),
    )
    .run();
}

/** Marks the row stored with what R2 holds; creates it if the start was never recorded. */
async function finish(env: Env, object: R2Object, entry: IndexEntry) {
  const stored = object.uploaded.toISOString();
  await env.REPORTS_INDEX.prepare(
    `INSERT INTO objects (key, kind, id, state, bytes, content_type, summary, metadata, started_at, stored_at)
     VALUES (?1, ?2, ?3, 'stored', ?4, ?5, ?6, ?7, ?8, ?8)
     ON CONFLICT (key) DO UPDATE SET state = 'stored', bytes = ?4, content_type = ?5, stored_at = ?8`,
  )
    .bind(
      object.key,
      entry.kind,
      entry.id,
      object.size,
      object.httpMetadata?.contentType ?? null,
      entry.summary ?? null,
      JSON.stringify(entry.metadata ?? {}),
      stored,
    )
    .run();
}

/** The only way to store an object in the private bucket. */
export async function putObject(
  env: Env,
  key: string,
  body: Parameters<R2Bucket['put']>[1],
  entry: IndexEntry,
  options?: R2PutOptions,
) {
  const contentType =
    options?.httpMetadata && !(options.httpMetadata instanceof Headers)
      ? options.httpMetadata.contentType
      : undefined;
  await begin(env, key, entry, contentType);
  const object = await env.ISSUE_REPORTS.put(key, body, options);
  if (!object) throw Error(`R2 did not store ${key}`);
  await finish(env, object, entry);
  return object;
}

/** Starts a multipart upload into the private bucket after recording it. */
export async function startUpload(
  env: Env,
  key: string,
  entry: IndexEntry,
  options?: R2MultipartOptions,
) {
  const contentType =
    options?.httpMetadata && !(options.httpMetadata instanceof Headers)
      ? options.httpMetadata.contentType
      : undefined;
  await begin(env, key, entry, contentType);
  return env.ISSUE_REPORTS.createMultipartUpload(key, options);
}

/** Completes a multipart upload and marks its row stored. */
export async function completeUpload(
  env: Env,
  upload: R2MultipartUpload,
  parts: R2UploadedPart[],
  entry: IndexEntry,
) {
  const object = await upload.complete(parts);
  await finish(env, object, entry);
  return object;
}

/** Marks an object R2 already holds as stored, such as an upload whose completion response was lost. */
export async function indexExisting(env: Env, object: R2Object, entry: IndexEntry) {
  await finish(env, object, entry);
}
