import { DurableObject } from 'cloudflare:workers';
import type { BudgetRecord, Env } from './env.ts';
import { completeUpload, indexExisting, putObject, startUpload } from './bucket.ts';

const MiB = 1024 * 1024;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[1-8][a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i;
const json = (value: object, status = 200) =>
  Response.json(value, { status, headers: { 'Cache-Control': 'no-store' } });
interface Identity {
  model: string;
  serial: string;
  name: string;
  bytes: number;
}
interface FileRecord extends Identity {
  key: string;
  uploadId?: string;
  complete: boolean;
  parts: (R2UploadedPart & { bytes: number })[];
}
interface Contribution {
  files: FileRecord[];
  finished: boolean;
}
const fileEntry = (id: string, file: FileRecord) => ({
  kind: 'contribution' as const,
  id,
  summary: file.name,
  metadata: { model: file.model, serial: file.serial, bytes: file.bytes },
});

export async function contributions(request: Request, env: Env) {
  const origin = request.headers.get('Origin') ?? '';
  if (
    !(env.ALLOWED_ORIGINS || '')
      .split(',')
      .map((s) => s.trim())
      .includes(origin)
  )
    return json({ error: 'Origin not allowed' }, 403);
  let response: Response;
  if (request.method === 'OPTIONS') response = new Response(null, { status: 204 });
  else if (
    !env.ISSUE_REPORTS ||
    !env.REPORTS_INDEX ||
    !env.CONTRIBUTIONS ||
    !env.ISSUE_LIMIT ||
    !env.CONTRIBUTION_LIMIT
  )
    response = json({ error: 'Uploads not configured' }, 503);
  else {
    const create =
      request.method === 'POST' && /^\/contributions\/[^/]+$/.test(new URL(request.url).pathname);
    const limiter = create ? env.ISSUE_LIMIT : env.CONTRIBUTION_LIMIT;
    if (!(await limiter.limit({ key: request.headers.get('CF-Connecting-IP') || 'local' })).success)
      response = json({ error: 'Upload rate exceeded' }, 429);
    else response = await env.CONTRIBUTIONS.getByName('contributions').fetch(request);
  }
  const headers = new Headers(response.headers);
  headers.set('Access-Control-Allow-Origin', origin);
  headers.set('Vary', 'Origin');
  headers.set('Access-Control-Allow-Methods', 'GET, POST, PUT');
  headers.set('Access-Control-Allow-Headers', 'Content-Type');
  return new Response(response.body, { status: response.status, headers });
}

class Invalid extends Error {
  constructor(
    message: string,
    readonly status = 400,
  ) {
    super(message);
  }
}
async function bounded(request: Request, maximum: number): Promise<Uint8Array<ArrayBuffer>> {
  const declared = request.headers.get('Content-Length');
  if (declared !== null && (!/^\d+$/.test(declared) || Number(declared) > maximum))
    throw new Invalid('Request too large', 413);
  const reader = request.body?.getReader();
  if (!reader) return new Uint8Array(0);
  // One bounded allocation avoids retaining chunks plus a second full-size copy.
  const buffer = new Uint8Array(declared === null ? maximum : Number(declared));
  let bytes = 0;
  for (;;) {
    const part = await reader.read();
    if (part.done) break;
    if (bytes + part.value.byteLength > buffer.byteLength) {
      await reader.cancel();
      throw new Invalid('Request too large', 413);
    }
    buffer.set(part.value, bytes);
    bytes += part.value.byteLength;
  }
  return buffer.subarray(0, bytes);
}
async function metadata(request: Request): Promise<unknown> {
  try {
    return JSON.parse(new TextDecoder().decode(await bounded(request, MiB)));
  } catch (error) {
    if (error instanceof Invalid) throw error;
    throw new Invalid('Invalid JSON');
  }
}
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Invalid('Invalid metadata');
  return value as Record<string, unknown>;
}
function segment(value: unknown): string {
  if (typeof value !== 'string' || !value.length || value.length > 200)
    throw new Invalid('Invalid file identity');
  const sanitized = value.replace(/[^A-Za-z0-9._-]/g, '_');
  if (sanitized === '.' || sanitized === '..') throw new Invalid('Invalid file identity');
  return sanitized;
}

/** A single coordinator makes creation and the daily budget atomic with respect to requests. */
export class Contributions extends DurableObject<Env> {
  async fetch(request: Request) {
    return this.ctx.blockConcurrencyWhile(async () => {
      try {
        return await this.intake(request);
      } catch (error) {
        if (error instanceof Invalid) return json({ error: error.message }, error.status);
        console.error('Contribution storage failed', error);
        return json({ error: 'Upload failed' }, 503);
      }
    });
  }
  private async intake(request: Request): Promise<Response> {
    const match =
      /^\/contributions\/([^/]+)(?:\/files\/(\d+)(?:\/(parts\/(\d+)|complete))?|\/(finish))?$/.exec(
        new URL(request.url).pathname,
      );
    if (!match || !uuid.test(match[1])) throw new Invalid('Invalid contribution path', 400);
    const id = match[1].toLowerCase(),
      key = `contribution:${id}`;
    let contribution = await this.ctx.storage.get<Contribution>(key);
    if (!match[2] && !match[5] && request.method === 'POST') {
      const body = record(await metadata(request));
      if (!Array.isArray(body.files) || !body.files.length || body.files.length > 256)
        throw new Invalid('Invalid files');
      const files = body.files.map((value): FileRecord => {
        const file = record(value);
        const model = segment(file.model),
          serial = segment(file.serial),
          name = segment(file.name);
        if (!Number.isSafeInteger(file.bytes) || (file.bytes as number) < 0)
          throw new Invalid('Invalid file size');
        return {
          model,
          serial,
          name,
          bytes: file.bytes as number,
          key: `models/${model}/cameras/${serial}/${id}/${name}.gz`,
          complete: false,
          parts: [],
        };
      });
      if (new Set(files.map((f) => f.key)).size !== files.length)
        throw new Invalid('Duplicate file identity');
      if (contribution) {
        if (
          JSON.stringify(
            contribution.files.map(({ model, serial, name, bytes }) => ({
              model,
              serial,
              name,
              bytes,
            })),
          ) !==
          JSON.stringify(
            files.map(({ model, serial, name, bytes }) => ({ model, serial, name, bytes })),
          )
        )
          throw new Invalid('Contribution files changed', 409);
        return json({ id, ...contribution });
      }
      const day = new Date().toISOString().slice(0, 10);
      const budget = await this.ctx.storage.get<BudgetRecord>('budget');
      const count = budget?.day === day ? budget.count : 0;
      if (count >= 50) throw new Invalid('Daily contribution limit reached', 429);
      contribution = { files, finished: false };
      await this.ctx.storage.put({ [key]: contribution, budget: { day, count: count + 1 } });
      return json({ id, ...contribution }, 201);
    }
    if (!contribution) throw new Invalid('Contribution not found', 404);
    if (!match[2] && !match[5] && request.method === 'GET') return json({ id, ...contribution });
    if (match[5] && request.method === 'POST') {
      if (contribution.finished) return json({ id });
      if (!contribution.files.every((file) => file.complete))
        throw new Invalid('Files incomplete', 409);
      const body = record(await metadata(request));
      if (
        !Object.hasOwn(body, 'manifest') ||
        typeof body.message !== 'string' ||
        body.message.length > 12000
      )
        throw new Invalid('Invalid manifest or message');
      const pendingKey = `finish:${id}`;
      const pending = (await this.ctx.storage.get<{ manifest: unknown; message: string }>(
        pendingKey,
      )) ?? { manifest: body.manifest, message: body.message };
      await this.ctx.storage.put(pendingKey, pending);
      const entry = {
        kind: 'contribution' as const,
        id,
        summary: pending.message.trim().split('\n')[0].slice(0, 200),
      };
      await putObject(
        this.env,
        `contributions/${id}/manifest.json`,
        JSON.stringify(pending.manifest),
        entry,
        { httpMetadata: { contentType: 'application/json' } },
      );
      await putObject(
        this.env,
        `contributions/${id}/message.json`,
        JSON.stringify({ message: pending.message }),
        entry,
        { httpMetadata: { contentType: 'application/json' } },
      );
      contribution.finished = true;
      await this.ctx.storage.put(key, contribution);
      await this.ctx.storage.delete(pendingKey);
      return json({ id });
    }
    const file = match[2] === undefined ? undefined : contribution.files[Number(match[2])];
    if (!file) throw new Invalid('File not found', 404);
    if (request.method !== 'POST' && request.method !== 'PUT')
      throw new Invalid('Method not allowed', 405);
    // Recover a completion whose R2 response or following durable write was interrupted.
    const stored =
      !file.complete && file.uploadId ? await this.env.ISSUE_REPORTS.head(file.key) : null;
    if (stored) {
      await indexExisting(this.env, stored, fileEntry(id, file));
      file.complete = true;
      await this.ctx.storage.put(key, contribution);
    }
    if (file.complete) return json(file);
    if (!match[3] && request.method === 'POST') {
      if (!file.uploadId) {
        const upload = await startUpload(this.env, file.key, fileEntry(id, file), {
          httpMetadata: { contentType: 'application/gzip' },
        });
        file.uploadId = upload.uploadId;
        await this.ctx.storage.put(key, contribution);
      }
      return json(file);
    }
    if (!file.uploadId) throw new Invalid('Start the file first', 409);
    const upload = this.env.ISSUE_REPORTS.resumeMultipartUpload(file.key, file.uploadId);
    if (match[4] && request.method === 'PUT') {
      const partNumber = Number(match[4]);
      if (!Number.isInteger(partNumber) || partNumber < 1 || partNumber > 10000)
        throw new Invalid('Invalid part number');
      const previous = file.parts.find((p) => p.partNumber === partNumber);
      if (previous) return json(previous);
      const body = await bounded(request, 64 * MiB);
      if (!body.byteLength) throw new Invalid('Empty part');
      const part = await upload.uploadPart(partNumber, body);
      const receipt = { ...part, bytes: body.byteLength };
      file.parts.push(receipt);
      file.parts.sort((a, b) => a.partNumber - b.partNumber);
      await this.ctx.storage.put(key, contribution);
      return json(receipt);
    }
    if (match[3] === 'complete' && request.method === 'POST') {
      const body = record(await metadata(request));
      if (
        body.parts !== file.parts.length ||
        !file.parts.length ||
        file.parts.some(
          (part, i, parts) =>
            part.partNumber !== i + 1 ||
            (i < parts.length - 1 && (part.bytes < 5 * MiB || part.bytes !== parts[0].bytes)),
        )
      )
        throw new Invalid('Invalid multipart sequence');
      await completeUpload(this.env, upload, file.parts, fileEntry(id, file));
      file.complete = true;
      await this.ctx.storage.put(key, contribution);
      return json(file);
    }
    throw new Invalid('Method not allowed', 405);
  }
}
