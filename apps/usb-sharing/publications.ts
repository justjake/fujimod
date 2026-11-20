import type { Env } from './env.ts';
import { fileError, filePage, htmlPage, serveObject } from './shared-files.ts';

interface PublishedFile {
  key: string;
  name: string;
}
const tokenPattern = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const escape = (text: string) =>
  text.replace(
    /[&<>"']/g,
    (char) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;',
      })[char]!,
  );

/** Call only after an explicit upload finishes. Retrying returns the same read-only link. */
export async function publishUpload(env: Env, id: string, files: PublishedFile[]): Promise<string> {
  const token = crypto.randomUUID();
  await env.REPORTS_INDEX.prepare(
    'INSERT INTO publications (id, token, files) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO NOTHING',
  )
    .bind(id, token, JSON.stringify(files))
    .run();
  const stored = await env.REPORTS_INDEX.prepare('SELECT token FROM publications WHERE id = ?1')
    .bind(id)
    .first<{ token: string }>();
  if (!stored) throw Error('Share link was not saved');
  const origin = (env.SHARING_ORIGIN || '').replace(/\/+$/, '');
  return `${origin}/contribution/${stored.token}`;
}

export async function publishedUpload(request: Request, env: Env): Promise<Response> {
  if (request.method !== 'GET' && request.method !== 'HEAD')
    return fileError('Method not allowed', 405, { Allow: 'GET, HEAD' });
  const url = new URL(request.url);
  const match = /^\/contribution\/([^/]+)(?:\/file\/(\d+)(?:\/(download))?)?$/.exec(url.pathname);
  if (!match || !tokenPattern.test(match[1])) return fileError('Not found', 404);
  const publication = await env.REPORTS_INDEX.prepare(
    'SELECT files FROM publications WHERE token = ?1',
  )
    .bind(match[1])
    .first<{ files: string }>();
  if (!publication) return fileError('Not found', 404);
  const files = JSON.parse(publication.files) as PublishedFile[];
  const base = `/contribution/${match[1]}`;
  let response: Response;
  if (match[2] !== undefined) {
    const index = Number(match[2]);
    const file = files[index];
    if (!Number.isSafeInteger(index) || !file) return fileError('Not found', 404);
    if (match[3]) {
      const download = await serveObject(request, env.ISSUE_REPORTS, file.key, true);
      download.headers.set('Cache-Control', 'no-store');
      return download;
    }
    const object = await env.ISSUE_REPORTS.head(file.key);
    if (!object) return fileError('Not found', 404);
    response = filePage(
      object,
      {
        page: `${base}/file/${index}`,
        media: `${base}/file/${index}/download`,
        download: `${base}/file/${index}/download`,
      },
      'download',
    );
  } else {
    response = htmlPage(
      `<ul class="shared-list">${files
        .map((file, index) => `<li><a href="${base}/file/${index}">${escape(file.name)}</a></li>`)
        .join('')}</ul>`,
      'Contributed files',
    );
  }
  // Capability URLs are revocable by deleting their publication. Do not cache the page.
  response.headers.set('Cache-Control', 'no-store');
  return request.method === 'HEAD' ? new Response(null, response) : response;
}
