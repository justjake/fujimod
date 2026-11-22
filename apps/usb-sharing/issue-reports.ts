import type { Env, BudgetRecord } from './env.ts';
import { DurableObject } from 'cloudflare:workers';
import { putObject } from './bucket.ts';
import { publishUpload } from './publications.ts';
import { parseReportMetadata, reportEntries } from '../web/src/bug/report-link.ts';
import { CONTACT_TYPES } from '../web/src/bug/general-report.ts';
import cameraLabelTable from './camera-labels.generated.json' with { type: 'json' };
const json = (value: object, status = 200) =>
  Response.json(value, { status, headers: { 'Cache-Control': 'no-store' } });
const MAX_CAMERA_BYTES = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES = MAX_CAMERA_BYTES + 128 * 1024;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
interface CameraLabel {
  name: string;
  description: string;
}
/** Generated from the catalog by `pnpm run generate`. */
const CAMERA_LABELS: Record<string, CameraLabel> = cameraLabelTable;
const GITHUB = 'https://api.github.com/repos/justjake/fujimod';
const github = (token: string) => ({
  Authorization: `Bearer ${token}`,
  Accept: 'application/vnd.github+json',
  'Content-Type': 'application/json',
  'User-Agent': 'FujiMod-Issue-Reports',
  'X-GitHub-Api-Version': '2022-11-28',
});
/** Create the camera's label with its description the first time that camera
 * reports, so GitHub never creates it bare. */
async function ensureLabel(token: string, label: CameraLabel) {
  const existing = await fetch(`${GITHUB}/labels/${encodeURIComponent(label.name)}`, {
    headers: github(token),
    signal: AbortSignal.timeout(5000),
  });
  if (existing.status !== 404) return;
  await fetch(`${GITHUB}/labels`, {
    method: 'POST',
    headers: github(token),
    body: JSON.stringify({ ...label, color: '1d76db' }),
    signal: AbortSignal.timeout(5000),
  });
}
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
async function parseSubmission(form: FormData) {
  const description = form.get('description');
  if (typeof description !== 'string' || description.length > 12000) throw Error();
  if (form.get('kind') === 'general') {
    const name = form.get('name') ?? '';
    const contactType = form.get('contact-type') ?? '';
    const contactValue = form.get('contact-value');
    const hasArchive = form.get('archive') === 'zip';
    if (
      typeof name !== 'string' ||
      name.length > 120 ||
      typeof contactType !== 'string' ||
      (contactType && !CONTACT_TYPES.some((type) => type === contactType)) ||
      (contactValue !== null && (typeof contactValue !== 'string' || contactValue.length > 254)) ||
      (!description.trim() && !hasArchive)
    )
      throw Error();
    const contact =
      contactType && contactValue?.trim()
        ? { type: contactType, value: contactValue.trim() }
        : undefined;
    return {
      kind: 'general' as const,
      description,
      name: name.trim(),
      contact,
      hasArchive,
      shareContact: !!contact && form.get('share-contact') === 'on',
      shareFiles: hasArchive && form.get('share-files') === 'on',
    };
  }
  const source = form.get('metadata');
  if (typeof source !== 'string' || source.length > 4096) throw Error();
  const metadata = parseReportMetadata(JSON.parse(source));
  const archive = form.get('report');
  if (
    metadata.archive ||
    !archive ||
    typeof archive === 'string' ||
    archive.size !== metadata.bytes ||
    archive.size > MAX_CAMERA_BYTES
  )
    throw Error();
  const bytes = new Uint8Array(await archive.arrayBuffer());
  const entries = reportEntries(bytes, metadata);
  if (entries.length > 64) throw Error();
  return { kind: 'camera' as const, description, metadata, bytes, entries };
}

/** Serializes intake and the global daily budget. Archives stay in a private R2
 * bucket; public issues contain an ID for maintainer retrieval, never a download URL.
 * Persisting `submitting` before GitHub prevents duplicate issues on uncertain replies. */
export class IssueReports extends DurableObject<Env> {
  private async receipt(id: string, issueURL?: string, general?: boolean) {
    if (general) return { id, issueURL };
    const shareUrl = await publishUpload(this.env, `issue:${id}`, [
      { key: `${id}/report.tar`, name: 'report.tar' },
      { key: `${id}/metadata.json`, name: 'metadata.json' },
    ]);
    return { id, issueURL, shareUrl };
  }
  async fetch(request: Request) {
    return this.ctx.blockConcurrencyWhile(async () => {
      const id = request.headers.get('Idempotency-Key');
      if (!id || !uuid.test(id)) return json({ error: 'Invalid request ID' }, 400);
      const declared = request.headers.get('Content-Length'),
        length = Number(declared);
      if (
        declared !== null &&
        (!Number.isSafeInteger(length) || length <= 0 || length > MAX_REQUEST_BYTES)
      )
        return json({ error: 'Invalid upload size' }, 413);
      const previous = await this.ctx.storage.get<{
        state: string;
        issueURL?: string;
        general?: boolean;
      }>(id);
      // Unconfirmed GitHub attempts are not automatically retried. The report
      // is durable, and its ID lets maintainers reconcile an ambiguous API reply.
      if (previous) return json(await this.receipt(id, previous.issueURL, previous.general));
      let submission;
      try {
        // Bound actual bytes as well as the declared length before parsing multipart.
        const reader = request.body!.getReader(),
          chunks = [];
        let total = 0;
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          total += part.value.length;
          if (total > MAX_REQUEST_BYTES) {
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
        submission = await parseSubmission(form);
        if (
          submission.kind === 'general' &&
          submission.hasArchive &&
          !(await this.env.ISSUE_REPORTS.head(`${id}/report.zip`))
        )
          return json({ error: 'Upload the ZIP archive first' }, 409);
      } catch {
        return json({ error: 'Invalid report submission' }, 400);
      }
      const day = new Date().toISOString().slice(0, 10),
        budget = await this.ctx.storage.get<BudgetRecord>('budget');
      const count = budget?.day === day ? budget.count : 0;
      if (count >= 50) return json({ error: 'Daily report limit reached' }, 429);
      await this.ctx.storage.put('budget', { day, count: count + 1 });
      const { description } = submission;
      const cameraReport = submission.kind === 'camera' ? submission : undefined;
      const metadata = cameraReport?.metadata;
      const entry = {
        kind: 'issue-report' as const,
        id,
        summary: description.trim().split('\n')[0].slice(0, 200),
        metadata:
          submission.kind === 'camera'
            ? {
                firmware: submission.metadata.firmware,
                commit: submission.metadata.commit,
                files: submission.entries.length,
              }
            : { archive: submission.hasArchive },
      };
      const received = new Date().toISOString();
      let storedMetadata;
      if (submission.kind === 'camera') {
        await putObject(this.env, `${id}/report.tar`, submission.bytes, entry, {
          httpMetadata: { contentType: 'application/x-tar' },
        });
        storedMetadata = { metadata, description, entries: submission.entries, received };
      } else {
        storedMetadata = {
          description,
          name: submission.name,
          contact: submission.contact,
          shareContact: submission.shareContact,
          shareFiles: submission.shareFiles,
          archive: submission.hasArchive ? 'report.zip' : undefined,
          received,
        };
      }
      await putObject(this.env, `${id}/metadata.json`, JSON.stringify(storedMetadata), entry, {
        httpMetadata: { contentType: 'application/json' },
      });
      const general = submission.kind === 'general';
      const shareUrl =
        general && submission.shareFiles
          ? await publishUpload(this.env, `issue:${id}`, [
              { key: `${id}/report.zip`, name: 'report.zip' },
            ])
          : undefined;
      await this.ctx.storage.put(id, { state: 'submitting', general });
      const camera = metadata && CAMERA_LABELS[metadata.firmware];
      const token = this.env.GITHUB_ISSUES_TOKEN ?? '';
      const publicDescription = description.trim().replaceAll('@', '@\u200b');
      const issueDetails = cameraReport
        ? `Firmware: ${cameraReport.metadata.firmware}\nFujiMod build: ${cameraReport.metadata.commit}\nReport ID: \`${id}\`\n\nDiagnostic archive stored privately for maintainers.\n\nFiles:\n${cameraReport.entries.map((e) => `- ${e.name} (${e.bytes} bytes)`).join('\n')}`
        : `Report ID: \`${id}\`\n\nZIP attachment: ${
            shareUrl
              ? `[Download uploaded files](${new URL(shareUrl, 'https://share.fujimod.com').href})`
              : general && submission.hasArchive
                ? 'stored privately'
                : 'none'
          }`;
      const publicContact =
        general && submission.shareContact && submission.contact
          ? `${submission.contact.type}: <code>${submission.contact.value
              .replaceAll('&', '&amp;')
              .replaceAll('<', '&lt;')
              .replaceAll('>', '&gt;')
              .replace(/[\r\n]/g, ' ')}</code>`
          : undefined;
      try {
        // Without its label GitHub would still apply the name, only without a description.
        if (camera) await ensureLabel(token, camera).catch((error) => console.error(error));
        const response = await fetch(`${GITHUB}/issues`, {
          method: 'POST',
          headers: github(token),
          body: JSON.stringify({
            title:
              (general ? '' : '[Camera] ') +
              (publicDescription.split('\n')[0] || 'Bug report').slice(0, 120),
            labels: general ? ['bug'] : ['bug', 'from-camera', ...(camera ? [camera.name] : [])],
            body: [publicDescription, issueDetails, publicContact].filter(Boolean).join('\n\n'),
          }),
          signal: AbortSignal.timeout(10000),
        });
        if (response.ok) {
          const issue = await response.json<{ html_url: string }>();
          if (
            /^https:\/\/github\.com\/justjake\/fujimod\/issues\/[1-9][0-9]*$/.test(issue.html_url)
          ) {
            await this.ctx.storage.put(id, {
              state: 'submitted',
              issueURL: issue.html_url,
              general,
            });
            return json(await this.receipt(id, issue.html_url, general), 201);
          }
        }
      } catch {
        /* Retain the stored report and the attempt for reconciliation. */
      }
      return json(await this.receipt(id, undefined, general), 202);
    });
  }
}
