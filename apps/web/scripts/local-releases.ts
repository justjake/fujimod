import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { Plugin, Connect } from 'vite';
import { validateIndex, type SdCardFile } from '../src/patch/catalog.ts';
import { releaseRoute } from '../src/patch/releases.ts';
import { sdBootV2Firmware, storeName, validateOtaRelease } from '../src/ota/release.ts';

interface LocalReleaseOptions {
  artifacts?: string;
  notes?: string;
  /** A directory holding `card.json` and its assets; its card files join the local release. */
  card?: string;
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
      const card: SdCardFile[] | undefined = options.card
        ? JSON.parse(await readFile(resolve(options.card, 'card.json'), 'utf8'))
        : undefined;
      const raw = {
        ...JSON.parse(await readFile(resolve(options.artifacts, 'patches.json'), 'utf8')),
        ...(card ? { sd_card_files: card } : {}),
      };
      const index = validateIndex(raw);
      const cardAssets = new Set(card?.map((file) => file.asset));
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
        const assets = new Set([
          'patches.json',
          ...Object.values(index.patches),
          ...(index.sd_card_files ?? []).map((file) => file.asset),
          ...(index.notes ? [index.notes.name] : []),
        ]);
        // Only serve manifests and store files named by this release's camera entries.
        if (route.tag === index.release && !assets.has(route.file))
          for (const camera of index.firmwares) {
            const firmware = sdBootV2Firmware(camera.id);
            const name = `${firmware}-latest.json`;
            let manifest: unknown;
            try {
              manifest = JSON.parse(await readFile(resolve(options.artifacts, name), 'utf8'));
            } catch (error) {
              if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue;
              throw error;
            }
            const release = validateOtaRelease(manifest, firmware);
            assets.add(name);
            for (const file of Object.values(release.fs)) assets.add(storeName(file.sha256));
          }
        if (route.tag !== index.release || !assets.has(route.file)) {
          response.statusCode = 404;
          response.end();
          return;
        }
        if (route.file === 'patches.json' && card) {
          data = Buffer.from(JSON.stringify(raw));
        } else if (cardAssets.has(route.file)) {
          data = await readFile(resolve(options.card!, route.file));
        } else if (options.notes && route.file === index.notes?.name) {
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
