import { createReadStream, existsSync, realpathSync } from 'node:fs';
import { realpath, stat } from 'node:fs/promises';
import { extname, isAbsolute, resolve, sep } from 'node:path';
import type { Connect, Plugin } from 'vite';

const types: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.txt': 'text/plain; charset=utf-8',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp',
  '.avif': 'image/avif',
  '.mp4': 'video/mp4',
  '.webm': 'video/webm',
  '.mov': 'video/quicktime',
  '.pdf': 'application/pdf',
  '.wasm': 'application/wasm',
};
const inside = (root: string, path: string) => path === root || path.startsWith(root + sep);

export function assertNoPublicReviews(webRoot: string): void {
  if (existsSync(resolve(webRoot, 'public/review')))
    throw Error(
      'Local review files must live outside the repository. Move public/review to the directory configured by FUJIMOD_REVIEW_DIR.',
    );
}

/** Dev-only mount. Its files never enter Vite's public directory or build output. */
export function reviewFiles(directory: string, repository: string): Connect.NextHandleFunction {
  if (!isAbsolute(directory))
    throw Error('FUJIMOD_REVIEW_DIR must be an absolute path outside the repository.');
  const root = realpathSync(directory);
  if (inside(realpathSync(repository), root))
    throw Error('FUJIMOD_REVIEW_DIR must be outside the repository.');
  return staticFiles(root, '/review/');
}

/** Filesystem mount shared by opt-in development artifact routes. */
export function staticFiles(
  directory: string,
  prefix: string,
  allowed: (path: string) => boolean = () => true,
): Connect.NextHandleFunction {
  const root = realpathSync(directory);
  return (req, res, next) => {
    const url = new URL(req.url ?? '/', 'http://localhost');
    if (!url.pathname.startsWith(prefix)) return next();
    const finish = (status: number) => {
      res.statusCode = status;
      res.end();
    };
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.setHeader('Allow', 'GET, HEAD');
      return finish(405);
    }
    const serve = async () => {
      let relative: string;
      try {
        relative = decodeURIComponent(url.pathname.slice(prefix.length));
      } catch {
        return finish(400);
      }
      if (relative.includes('\0') || relative.split(/[\\/]/).some((part) => part === '..'))
        return finish(403);
      let path = resolve(root, relative);
      if (!inside(root, path)) return finish(403);
      if (!allowed(path)) return next();
      try {
        path = await realpath(path);
        if (!inside(root, path) || !allowed(path)) return finish(403);
        let info = await stat(path);
        if (info.isDirectory()) {
          if (!url.pathname.endsWith('/')) {
            res.setHeader('Location', url.pathname + '/' + url.search);
            return finish(302);
          }
          path = await realpath(resolve(path, 'index.html'));
          if (!inside(root, path)) return finish(403);
          info = await stat(path);
        }
        if (!info.isFile()) return finish(404);
        let start = 0;
        let end = info.size - 1;
        if (req.headers.range && req.method === 'GET') {
          const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range);
          if (range && (range[1] || range[2])) {
            start = range[1] ? Number(range[1]) : Math.max(0, info.size - Number(range[2]));
            end = range[1] && range[2] ? Math.min(Number(range[2]), end) : end;
          }
          if (
            !range ||
            (!range[1] && !range[2]) ||
            !info.size ||
            !Number.isSafeInteger(start) ||
            !Number.isSafeInteger(end) ||
            start > end ||
            end >= info.size ||
            (!range[1] && Number(range[2]) === 0)
          ) {
            res.setHeader('Content-Range', `bytes */${info.size}`);
            return finish(416);
          }
          res.statusCode = 206;
          res.setHeader('Content-Range', `bytes ${start}-${end}/${info.size}`);
        }
        res.setHeader(
          'Content-Type',
          types[extname(path).toLowerCase()] ?? 'application/octet-stream',
        );
        res.setHeader('Content-Length', String(Math.max(0, end - start + 1)));
        res.setHeader('Cache-Control', 'no-store');
        res.setHeader('X-Content-Type-Options', 'nosniff');
        res.setHeader('Accept-Ranges', 'bytes');
        if (req.method === 'HEAD' || !info.size) return res.end();
        createReadStream(path, { start, end })
          .on('error', () => res.destroy())
          .pipe(res);
      } catch (error) {
        if (
          error instanceof Error &&
          'code' in error &&
          (error.code === 'ENOENT' || error.code === 'ENOTDIR')
        )
          return finish(404);
        next(error);
      }
    };
    void serve().catch(next);
  };
}

export function localReviewFiles(directory: string | undefined): Plugin {
  return {
    name: 'local-review-files',
    configResolved(config) {
      assertNoPublicReviews(config.root);
    },
    configureServer(server) {
      if (directory)
        server.middlewares.use(reviewFiles(directory, resolve(server.config.root, '../..')));
    },
  };
}
