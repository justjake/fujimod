/** Opt-in development media, served directly from a recording workspace. */
import { existsSync, realpathSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import type { Connect, Plugin } from 'vite';
import { staticFiles } from './local-review-files.ts';

export function previewMedia(
  root: string | undefined,
  fallback: string | undefined,
): string | undefined {
  if (root) {
    const frontier = resolve(root, 'media/frontier/site');
    if (existsSync(resolve(frontier, 'media.json'))) return frontier;
    const current = resolve(root, 'media/current');
    if (existsSync(resolve(current, 'media.json'))) return current;
  }
  return fallback;
}

export function recordingFiles(directory: string): Connect.NextHandleFunction {
  const root = realpathSync(directory);
  const allowed = (path: string): boolean => {
    const name = relative(root, path);
    if (name.split(sep).some((part) => part === '.inputs' || part === '.history')) return false;
    if (/^media\/(?:frontier|current|builds\/[^/]+)\//.test(name)) return true;
    if (!/\.(?:mp4|webm|png|webp|json|svg)$/.test(name)) return false;
    for (
      let parent = dirname(path);
      parent !== root && parent.startsWith(root + sep);
      parent = dirname(parent)
    )
      if (
        existsSync(resolve(parent, 'guide.json')) ||
        existsSync(resolve(parent, '.inputs/receipt.json'))
      )
        return true;
    return false;
  };
  return staticFiles(root, '/', allowed);
}

export function localMedia(root: string | undefined, fallback: string | undefined): Plugin {
  return {
    name: 'local-media',
    apply: 'serve',
    configureServer(server) {
      if (!root) return;
      server.middlewares.use((req, res, next) => {
        if (!req.url?.startsWith('/artifacts/docs/')) return next();
        const media = previewMedia(root, fallback);
        if (!media) return next();
        return staticFiles(media, '/artifacts/docs/')(req, res, next);
      });
      server.middlewares.use(recordingFiles(root));
      // Rebuild the embedded homepage manifest when a pass lands or promotion
      // renames the frontier. Both SSR and browser modules must use one revision.
      const mediaRoot = resolve(root, 'media');
      server.watcher.add(mediaRoot);
      let pending: ReturnType<typeof setTimeout> | undefined;
      const update = (path: string) => {
        if (!path.startsWith(mediaRoot + sep) || !path.endsWith('/site/media.json')) return;
        clearTimeout(pending);
        pending = setTimeout(() => {
          void server.restart();
        }, 150);
      };
      server.watcher.on('all', (_event, path) => update(path));
      server.httpServer?.once('close', () => clearTimeout(pending));
    },
  };
}
