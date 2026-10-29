import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { Plugin, Connect } from 'vite';
import { validateIndex } from '../src/patch/catalog.ts';
import { releaseRoute } from '../src/patch/releases.ts';

interface LocalReleaseOptions {
  artifacts?: string;
  notes?: string;
}

/** Editable notes are served directly from source on every request. */
export function localReleaseMiddleware(options: LocalReleaseOptions): Connect.NextHandleFunction {
  return async (request, response, next) => {
    const path = new URL(request.url || '/', 'http://localhost').pathname;
    if (!path.startsWith('/releases/')) return next();
    response.setHeader('Cache-Control', 'no-store');
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      response.statusCode = 405;
      response.setHeader('Allow', 'GET, HEAD');
      response.end();
      return;
    }
    const route = releaseRoute(path);
    if (!options.artifacts || !route) {
      response.statusCode = 404;
      response.end();
      return;
    }
    try {
      const index = validateIndex(
        JSON.parse(await readFile(resolve(options.artifacts, 'patches.json'), 'utf8')),
      );
      let data: Buffer;
      if (route.kind === 'current') {
        data = Buffer.from(
          JSON.stringify({
            tag: index.release,
            name: index.release,
            published_at: index.built_utc || '1970-01-01T00:00:00Z',
            prerelease: true,
          }),
        );
        response.setHeader('Content-Type', 'application/json');
      } else {
        if (
          route.tag !== index.release ||
          !new Set([
            'patches.json',
            ...Object.values(index.patches),
            ...(index.notes ? [index.notes.name] : []),
          ]).has(route.file)
        ) {
          response.statusCode = 404;
          response.end();
          return;
        }
        if (options.notes && route.file === index.notes?.name) {
          data = await readFile(resolve(options.notes, `${index.release}.md`));
        } else {
          data = await readFile(resolve(options.artifacts, route.file));
        }
        response.setHeader(
          'Content-Type',
          route.file.endsWith('.json')
            ? 'application/json'
            : route.file.endsWith('.md')
              ? 'text/markdown; charset=utf-8'
              : 'application/octet-stream',
        );
      }
      response.setHeader('Content-Length', data.length);
      response.end(request.method === 'HEAD' ? undefined : data);
    } catch (error) {
      response.statusCode = (error as NodeJS.ErrnoException).code === 'ENOENT' ? 404 : 500;
      response.end();
    }
  };
}
export function localReleases(options: LocalReleaseOptions): Plugin {
  const configure = (middlewares: Connect.Server) => {
    middlewares.use(localReleaseMiddleware(options));
  };
  return {
    name: 'local-releases',
    apply: 'serve',
    configureServer: (server) => configure(server.middlewares),
    configurePreviewServer: (server) => configure(server.middlewares),
  };
}
