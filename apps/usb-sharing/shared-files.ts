import type { Env } from './env.ts';
import { sharedFilesStyle } from './shared-files-style.ts';

const origin = 'https://share.fujimod.com';
const pagePolicy =
  "default-src 'none'; style-src 'self'; img-src 'self'; media-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const escape = (value: string) =>
  value.replace(
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
const encodedKey = (key: string) => key.split('/').map(encodeURIComponent).join('/');
const filename = (key: string) => key.slice(key.lastIndexOf('/') + 1) || 'download';

/** True for C0 control characters and DEL, which no share path may contain. */
function hasControlCharacter(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}
export function fileError(message: string, status: number, extra?: HeadersInit): Response {
  const headers = new Headers(extra);
  headers.set('Content-Type', 'text/plain; charset=utf-8');
  headers.set('Cache-Control', 'no-store');
  headers.set('X-Content-Type-Options', 'nosniff');
  return new Response(message, { status, headers });
}

function mediaType(object: R2Object): string | undefined {
  const type = object.httpMetadata?.contentType;
  const essence = type?.split(';')[0].trim();
  return type &&
    !/[\r\n]/.test(type) &&
    /^(image|video)\/[!#$%&'*+.^_`|~a-z0-9-]+$/i.test(essence ?? '')
    ? type
    : undefined;
}

export function htmlPage(body: string, title: string, meta = ''): Response {
  return new Response(
    `<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="dark"><title>${escape(title)} · FujiMod</title>
<link rel="stylesheet" href="/file-viewer.css">${meta}</head>
<body><main class="viewer" aria-label="Shared file">${body}</main></body></html>`,
    {
      headers: {
        'Content-Type': 'text/html; charset=utf-8',
        'Cache-Control': 'no-cache',
        'Content-Security-Policy': pagePolicy,
        'X-Content-Type-Options': 'nosniff',
        'Referrer-Policy': 'no-referrer',
      },
    },
  );
}

export function filePage(
  object: R2Object,
  paths: { page: string; media: string; download: string; poster?: string },
  requested: string | null,
): Response {
  const name = filename(object.key);
  const type = mediaType(object);
  const kind = type?.split('/')[0].toLowerCase();
  const preview = requested !== 'download' && kind && (!requested || requested === kind);
  const tag = (key: string, value: string) => `<meta property="${key}" content="${escape(value)}">`;
  let meta =
    tag('og:title', name) + tag('og:site_name', 'FujiMod') + tag('og:url', origin + paths.page);
  let media = '';
  if (preview && kind === 'image') {
    media = `<section class="media"><img src="${escape(paths.media)}" alt="${escape(name)}"></section>`;
    meta +=
      tag('og:type', 'website') +
      tag('og:image', origin + paths.media) +
      tag('og:image:type', type!) +
      tag('og:image:alt', name) +
      '<meta name="twitter:card" content="summary_large_image">';
  } else if (preview && kind === 'video') {
    media = `<section class="media"><video controls playsinline preload="metadata" src="${escape(paths.media)}"${paths.poster ? ` poster="${escape(paths.poster)}"` : ''} aria-label="${escape(name)}"></video></section>`;
    if (paths.poster)
      meta +=
        tag('og:type', 'video.other') +
        tag('og:video', origin + paths.media) +
        tag('og:video:type', type!) +
        tag('og:image', origin + paths.poster) +
        tag('og:image:alt', name) +
        '<meta name="twitter:card" content="player">';
  }
  const metadata = object.httpMetadata;
  const rows: [string, string | undefined][] = [
    ['Content-Type', metadata?.contentType ?? 'application/octet-stream'],
    ['Content-Disposition', metadata?.contentDisposition],
    ['Content-Encoding', metadata?.contentEncoding],
    ['Content-Language', metadata?.contentLanguage],
    ['Cache-Control', metadata?.cacheControl],
    ['Expires', metadata?.cacheExpiry?.toUTCString()],
    ['Size', `${object.size.toLocaleString('en-US')} bytes`],
    ['Uploaded', object.uploaded.toISOString()],
    ['ETag', object.httpEtag],
    ...Object.entries(object.customMetadata ?? {}).map(([key, value]): [string, string] => [
      `x-amz-meta-${key}`,
      value,
    ]),
  ];
  const table = rows
    .filter((row): row is [string, string] => row[1] !== undefined)
    .map(([key, value]) => `<tr><th scope="row">${escape(key)}</th><td>${escape(value)}</td></tr>`)
    .join('');
  return htmlPage(
    `${media}<h1>${escape(name)}</h1>
<table aria-label="File metadata and upload headers"><tbody>${table}</tbody></table>
<a class="download" href="${escape(paths.download)}" download>Download</a>`,
    name,
    meta,
  );
}

function disposition(name: string): string {
  const encoded = encodeURIComponent(name).replace(
    /[!'()*]/g,
    (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  );
  return `attachment; filename="download"; filename*=UTF-8''${encoded}`;
}

function range(header: string, size: number): { offset: number; length: number } | null {
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || (!match[1] && !match[2]) || !size) return null;
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) return null;
    const length = Math.min(suffix, size);
    return { offset: size - length, length };
  }
  const offset = Number(match[1]);
  const end = match[2] ? Number(match[2]) : size - 1;
  if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(end) || offset >= size || end < offset)
    return null;
  return { offset, length: Math.min(end, size - 1) - offset + 1 };
}

/** Only image/video MIME types render inline; response policy stays controlled here. */
export async function serveObject(
  request: Request,
  bucket: R2Bucket,
  key: string,
  download: boolean,
): Promise<Response> {
  const object = await bucket.head(key);
  if (!object) return fileError('Not found', 404);
  const type = mediaType(object);
  if (!download && !type) return fileError('File is available as a download', 415);
  const headers = new Headers({
    'Content-Type': download ? 'application/octet-stream' : type!,
    'Content-Disposition': download ? disposition(filename(key)) : 'inline',
    'X-Content-Type-Options': 'nosniff',
    // A sandbox blocks WebKit's native video controls. Other responses stay sandboxed.
    'Content-Security-Policy':
      !download && type?.toLowerCase().startsWith('video/')
        ? "default-src 'none'; media-src 'self'"
        : "default-src 'none'; sandbox",
    'Referrer-Policy': 'no-referrer',
    'Cache-Control': 'public, max-age=300',
    'Accept-Ranges': 'bytes',
    ETag: object.httpEtag,
    'Last-Modified': object.uploaded.toUTCString(),
  });
  if (!download && object.httpMetadata?.contentEncoding)
    headers.set('Content-Encoding', object.httpMetadata.contentEncoding);
  const noneMatch = request.headers.get('If-None-Match');
  const modified = Date.parse(request.headers.get('If-Modified-Since') ?? '');
  if (
    noneMatch
      ? noneMatch
          .split(',')
          .some(
            (value) => value.trim() === '*' || value.trim().replace(/^W\//, '') === object.httpEtag,
          )
      : Number.isFinite(modified) &&
        Math.floor(object.uploaded.getTime() / 1000) <= Math.floor(modified / 1000)
  )
    return new Response(null, { status: 304, headers });
  let part: { offset: number; length: number } | undefined;
  const rangeHeader = request.method === 'GET' ? request.headers.get('Range') : null;
  const ifRange = request.headers.get('If-Range');
  const useRange =
    !ifRange ||
    ifRange === object.httpEtag ||
    (!ifRange.startsWith('W/') &&
      Math.floor(object.uploaded.getTime() / 1000) <= Date.parse(ifRange) / 1000);
  if (rangeHeader && useRange) {
    const parsed = range(rangeHeader, object.size);
    if (!parsed)
      return fileError('Range not satisfiable', 416, { 'Content-Range': `bytes */${object.size}` });
    part = parsed;
    headers.set(
      'Content-Range',
      `bytes ${part.offset}-${part.offset + part.length - 1}/${object.size}`,
    );
  }
  headers.set('Content-Length', String(part?.length ?? object.size));
  if (request.method === 'HEAD') return new Response(null, { headers });
  const result = await bucket.get(key, { range: part, onlyIf: { etagMatches: object.etag } });
  if (!result) return fileError('Not found', 404);
  if (!('body' in result)) return fileError('File changed; retry the request', 412);
  return new Response(result.body, { status: part ? 206 : 200, headers });
}

export async function sharedFiles(request: Request, env: Env): Promise<Response> {
  if (request.method !== 'GET' && request.method !== 'HEAD')
    return fileError('Method not allowed', 405, { Allow: 'GET, HEAD' });
  const url = new URL(request.url);
  if (url.pathname === '/file-viewer.css')
    return new Response(request.method === 'HEAD' ? null : sharedFilesStyle, {
      headers: {
        'Content-Type': 'text/css; charset=utf-8',
        'Cache-Control': 'public, max-age=3600',
        'X-Content-Type-Options': 'nosniff',
      },
    });
  const match = /^\/(file|media|download)\/(.+)$/.exec(url.pathname);
  if (!match) return fileError('Not found', 404);
  let key: string;
  try {
    key = decodeURIComponent(match[2]);
  } catch {
    return fileError('Invalid file path', 400);
  }
  if (
    !key ||
    key.startsWith('/') ||
    key.split('/').some((segment) => segment === '.' || segment === '..') ||
    hasControlCharacter(key)
  )
    return fileError('Invalid file path', 400);
  if (match[1] === 'file' && url.searchParams.get('direct') === 'true')
    return serveObject(request, env.SHARED_FILES, key, false);
  if (match[1] !== 'file')
    return serveObject(request, env.SHARED_FILES, key, match[1] === 'download');
  const object = await env.SHARED_FILES.head(key);
  if (!object) return fileError('Not found', 404);
  const path = encodedKey(key);
  let poster: string | undefined;
  if (mediaType(object)?.toLowerCase().startsWith('video/')) {
    const thumbnail = await env.SHARED_FILES.head(`${key}.poster.png`);
    if (thumbnail && mediaType(thumbnail) === 'image/png') poster = `/media/${path}.poster.png`;
  }
  const response = filePage(
    object,
    {
      page: `/file/${path}${url.search}`,
      media: `/media/${path}`,
      download: `/download/${path}`,
      poster,
    },
    url.searchParams.get('type'),
  );
  return request.method === 'HEAD' ? new Response(null, response) : response;
}
