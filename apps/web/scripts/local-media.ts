/** Opt-in development media, served directly from a recording workspace. */
import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import type { Connect, Plugin } from 'vite';
import { staticFiles } from './local-review-files.ts';
import { featureMediaFiles } from './stage-site.ts';
import { validateFeatureMedia } from '../src/home/feature-media.ts';
import homepage from '../src/home/features.generated.json' with { type: 'json' };

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

/** Embed one manifest and publish its referenced recordings from the same directory. */
export function siteFeatureMedia(root: string | undefined, configured: string | undefined) {
  const staged = resolve('public/artifacts/docs');
  let directory: string | undefined;
  let manifest: ReturnType<typeof validateFeatureMedia> | null;
  return {
    name: 'site-feature-media',
    config(_config, { command }) {
      directory = (command === 'serve' ? previewMedia(root, configured) : configured) ?? staged;
      const path = resolve(directory, 'media.json');
      if (existsSync(path)) {
        directory = realpathSync(directory);
        manifest = validateFeatureMedia(
          JSON.parse(readFileSync(resolve(directory, 'media.json'), 'utf8')),
        );
      } else {
        manifest = null;
      }
      return { define: { __FUJIMOD_FEATURE_MEDIA__: JSON.stringify(manifest) } };
    },
    writeBundle(options) {
      if (this.environment.name !== 'client' || !directory || !manifest || !options.dir) return;
      if (existsSync(staged) && directory === realpathSync(staged)) return;
      const destination = resolve(options.dir, 'artifacts/docs');
      mkdirSync(destination, { recursive: true });
      const names = featureMediaFiles(manifest, Object.keys(homepage.cards));
      for (const name of [...names, 'media.json'])
        copyFileSync(resolve(directory, name), resolve(destination, name));
    },
  } satisfies Plugin;
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
