import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import type { Plugin } from 'vite';
import { staticFiles } from './local-review-files.ts';

/** Where the home page's live Film Editor is served. */
export const filmEditorDemoPath = 'demo/film-editor';

const file = /^[A-Za-z0-9_.-]+$/;

/** A demo build: the flat file names the page loads, and whether it may
 * only be served in development. */
export interface FilmEditorDemo {
  files: string[];
  developmentOnly: boolean;
}

/** The demo's manifest.json: schema 1, the flat file names the page loads and
 * an optional `developmentOnly` flag. */
export function parseFilmEditorDemo(json: string): FilmEditorDemo {
  const manifest = JSON.parse(json) as {
    schema?: unknown;
    files?: unknown;
    developmentOnly?: unknown;
  } | null;
  if (manifest?.schema !== 1 || !Array.isArray(manifest.files))
    throw Error('The Film Editor demo manifest has an unknown schema.');
  const files = manifest.files as unknown[];
  if (!files.includes('index.html'))
    throw Error('The Film Editor demo manifest has no index.html.');
  for (const name of files)
    if (typeof name !== 'string' || !file.test(name) || name.startsWith('.'))
      throw Error(`The Film Editor demo manifest names an invalid file: ${String(name)}`);
  if (manifest.developmentOnly !== undefined && typeof manifest.developmentOnly !== 'boolean')
    throw Error('The Film Editor demo manifest has an invalid developmentOnly flag.');
  return { files: files as string[], developmentOnly: manifest.developmentOnly === true };
}

/** Serves FUJIMOD_FILM_EDITOR_DEMO in development and copies it into the build.
 * A development-only demo is refused by every build but the local preview. */
export function siteFilmEditorDemo(configured: string | undefined, localPreview: boolean): Plugin {
  let directory: string | undefined;
  let files: string[] = [];
  return {
    name: 'site-film-editor-demo',
    config(_config, env) {
      if (configured) {
        const manifest = resolve(configured, 'manifest.json');
        if (!existsSync(manifest))
          throw Error(`FUJIMOD_FILM_EDITOR_DEMO has no manifest.json: ${configured}`);
        const demo = parseFilmEditorDemo(readFileSync(manifest, 'utf8'));
        if (demo.developmentOnly && env.command === 'build' && !localPreview)
          throw Error(
            `FUJIMOD_FILM_EDITOR_DEMO is a development-only build and cannot be published: ${configured}`,
          );
        files = demo.files;
        directory = realpathSync(configured);
      }
      return { define: { __FUJIMOD_FILM_EDITOR_DEMO__: JSON.stringify(!!directory) } };
    },
    configureServer(server) {
      if (!directory) return;
      const source = directory;
      const allowed = new Set(files.map((name) => resolve(source, name)));
      server.middlewares.use(
        staticFiles(source, `/${filmEditorDemoPath}/`, (path) => allowed.has(path)),
      );
    },
    writeBundle(options) {
      if (this.environment.name !== 'client' || !directory || !options.dir) return;
      const destination = resolve(options.dir, filmEditorDemoPath);
      for (const name of files) {
        const target = resolve(destination, name);
        mkdirSync(dirname(target), { recursive: true });
        copyFileSync(resolve(directory, name), target);
      }
    },
  };
}
