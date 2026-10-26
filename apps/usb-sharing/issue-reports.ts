import type { Env, BudgetRecord } from './env.ts';
import { DurableObject } from 'cloudflare:workers';
import { putObject } from './bucket.ts';
import { parseReportMetadata, reportEntries } from '../web/src/bug/report-link.ts';
const json = (value: object, status = 200) =>
  Response.json(value, { status, headers: { 'Cache-Control': 'no-store' } });
const MAX_UPLOAD = 8 * 1024 * 1024;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
export async function issueReports(request: Request, env: Env) {
  const origin = request.headers.get('Origin');
  if (
    !(env.ALLOWED_ORIGINS || '')
      .split(',')
      .map((s) => s.trim())
      .includes(origin ?? '')
  )
    return json({ error: 'Origin not allowed' }, 403);
  let response;
  if (request.method === 'OPTIONS') response = new Response(null, { status: 204 });
  else if (request.method !== 'POST') response = json({ error: 'POST required' }, 405);
  else if (!env.ISSUE_REPORTS || !env.GITHUB_ISSUES_TOKEN || !env.ISSUES || !env.ISSUE_LIMIT)
    response = json({ error: 'Uploads not configured' }, 503);
  else if (!uuid.test(request.headers.get('Idempotency-Key') || ''))
    response = json({ error: 'Invalid request ID' }, 400);
  else if (
    !(await env.ISSUE_LIMIT.limit({ key: request.headers.get('CF-Connecting-IP') || 'local' }))
      .success
  )
    response = json({ error: 'Upload rate exceeded' }, 429);
  else response = await env.ISSUES.getByName('reports').fetch(request);
  const headers = new Headers(response.headers);
  headers.set('Access-Control-Allow-Origin', origin ?? '');
  headers.set('Vary', 'Origin');
  headers.set('Access-Control-Allow-Methods', 'POST');
  headers.set('Access-Control-Allow-Headers', 'Content-Type, Idempotency-Key');
  return new Response(response.body, { status: response.status, headers });
}
/** Serializes intake and the global daily budget. Archives stay in a private R2
 * bucket; public issues contain an ID for maintainer retrieval, never a download URL.
 * Persisting `submitting` before GitHub prevents duplicate issues on uncertain replies. */
export class IssueReports extends DurableObject<Env> {
  async fetch(request: Request) {
    return this.ctx.blockConcurrencyWhile(async () => {
      const id = request.headers.get('Idempotency-Key');
      if (!id || !uuid.test(id)) return json({ error: 'Invalid request ID' }, 400);
      const declared = request.headers.get('Content-Length'),
        length = Number(declared);
      if (
        declared !== null &&
        (!Number.isSafeInteger(length) || length < 1024 || length > MAX_UPLOAD + 32768)
      )
        return json({ error: 'Invalid upload size' }, 413);
      const previous = await this.ctx.storage.get<{ state: string; issueURL?: string }>(id);
      // Unconfirmed GitHub attempts are not automatically retried. The archive
      // is durable, and its ID lets maintainers reconcile an ambiguous API reply.
      if (previous) return json({ id, issueURL: previous.issueURL });
      let metadata, description, archive, entries;
      try {
        // Bound actual bytes as well as the declared length before parsing multipart.
        const reader = request.body!.getReader(),
          chunks = [];
        let total = 0;
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          total += part.value.length;
          if (total > MAX_UPLOAD + 32768) {
            await reader.cancel();
            return json({ error: 'Upload too large' }, 413);
          }
          chunks.push(part.value);
        }
        const body = new Blob(chunks);
        const form = await new Request(request.url, {
          method: 'POST',
          headers: request.headers,
          body,
        }).formData();
        const source = form.get('metadata');
        if (typeof source !== 'string' || source.length > 4096) throw Error();
        metadata = parseReportMetadata(JSON.parse(source));
        if (metadata.archive) throw Error();
        description = form.get('description');
        archive = form.get('report');
        if (
          typeof description !== 'string' ||
          description.length > 12000 ||
          !archive ||
          typeof archive === 'string' ||
          archive.size !== metadata.bytes ||
          archive.size > MAX_UPLOAD
        )
          throw Error();
        archive = await archive.arrayBuffer();
        entries = reportEntries(new Uint8Array(archive), metadata);
        if (entries.length > 64) throw Error();
      } catch {
        return json({ error: 'Invalid report archive' }, 400);
      }
      const day = new Date().toISOString().slice(0, 10),
        budget = await this.ctx.storage.get<BudgetRecord>('budget');
      const count = budget?.day === day ? budget.count : 0;
      if (count >= 50) return json({ error: 'Daily report limit reached' }, 429);
      await this.ctx.storage.put('budget', { day, count: count + 1 });
      const entry = {
        kind: 'issue-report' as const,
        id,
        summary: description.trim().split('\n')[0].slice(0, 200),
        metadata: { firmware: metadata.firmware, commit: metadata.commit, files: entries.length },
      };
      await putObject(this.env, `${id}/report.tar`, archive, entry, {
        httpMetadata: { contentType: 'application/x-tar' },
      });
      await putObject(
        this.env,
        `${id}/metadata.json`,
        JSON.stringify({ metadata, description, entries, received: new Date().toISOString() }),
        entry,
        { httpMetadata: { contentType: 'application/json' } },
      );
      await this.ctx.storage.put(id, { state: 'submitting' });
      try {
        const response = await fetch('https://api.github.com/repos/justjake/fujimod/issues', {
          method: 'POST',
          headers: {
            Authorization: `Bearer ${this.env.GITHUB_ISSUES_TOKEN}`,
            Accept: 'application/vnd.github+json',
            'Content-Type': 'application/json',
            'User-Agent': 'FujiMod-Issue-Reports',
            'X-GitHub-Api-Version': '2022-11-28',
          },
          body: JSON.stringify({
            title: '[Camera] ' + (description.trim().split('\n')[0] || 'Bug report').slice(0, 120),
            body: `${description.trim().replaceAll('@', '@\u200b')}\n\nFirmware: ${metadata.firmware}\nFujiMod build: ${metadata.commit}\nReport ID: \`${id}\`\n\nDiagnostic archive stored privately for maintainers.\n\nFiles:\n${entries.map((e) => `- ${e.name} (${e.bytes} bytes)`).join('\n')}`,
          }),
          signal: AbortSignal.timeout(10000),
        });
        if (response.ok) {
          const issue = await response.json<{ html_url: string }>();
          if (
            /^https:\/\/github\.com\/justjake\/fujimod\/issues\/[1-9][0-9]*$/.test(issue.html_url)
          ) {
            await this.ctx.storage.put(id, { state: 'submitted', issueURL: issue.html_url });
            return json({ id, issueURL: issue.html_url }, 201);
          }
        }
      } catch {
        /* Retain the durable archive and the attempt for reconciliation. */
      }
      return json({ id }, 202);
    });
  }
}
