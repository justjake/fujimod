import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import type { Plugin } from 'vite';
import type { FocalSdkDocs } from '../src/docs/sdk-data.ts';
import { staticFiles } from './local-review-files.ts';

export const sdkDocsPath = 'docs/sdk/focal';
export const sdkDocsManifest = 'manifest.json';

const slug = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const previewAssets = [
  'preview.html',
  'preview.page.js',
  'preview.wasm',
  'preview.wasm.js',
  'focal-runtime.js',
  'web_glyphs.js',
];

export function parseSdkDocs(json: string): FocalSdkDocs {
  const docs = JSON.parse(json) as FocalSdkDocs | null;
  if (docs?.schema !== 1 || !Array.isArray(docs.components))
    throw Error('FocalUI SDK docs have an unknown schema.');
  const components = new Set<string>();
  for (const component of docs.components) {
    if (
      typeof component?.slug !== 'string' ||
      !slug.test(component.slug) ||
      components.has(component.slug) ||
      !Array.isArray(component.declarations) ||
      !Array.isArray(component.previews)
    )
      throw Error('Invalid FocalUI SDK component.');
    components.add(component.slug);
    const previews = new Set<string>();
    for (const preview of component.previews) {
      if (
        typeof preview?.slug !== 'string' ||
        !slug.test(preview.slug) ||
        previews.has(preview.slug) ||
        preview.page !== `/${sdkDocsPath}/${component.slug}/${preview.slug}/preview.html`
      )
        throw Error(`Invalid FocalUI SDK preview: ${component.slug}`);
      previews.add(preview.slug);
    }
  }
  return docs;
}

export function sdkDocsFiles(docs: FocalSdkDocs): string[] {
  return [
    sdkDocsManifest,
    ...docs.components.flatMap((component) =>
      component.previews.flatMap((preview) =>
        previewAssets.map((name) => `${component.slug}/${preview.slug}/${name}`),
      ),
    ),
  ];
}

export function readSdkDocs(directory: string): FocalSdkDocs | undefined {
  const manifest = resolve(directory, sdkDocsManifest);
  if (existsSync(manifest)) return parseSdkDocs(readFileSync(manifest, 'utf8'));
}

export function siteSdkDocs(configured: string | undefined): Plugin {
  let directory: string | undefined;
  let files = new Set<string>();
  return {
    name: 'site-sdk-docs',
    config() {
      const staged = resolve('public', sdkDocsPath);
      const source = configured ?? staged;
      const docs = readSdkDocs(source);
      if (configured && !docs) throw Error(`FUJIMOD_SDK_DOCS has no ${sdkDocsManifest}: ${source}`);
      if (docs) {
        directory = realpathSync(source);
        files = new Set(sdkDocsFiles(docs));
      }
      return { define: { __FUJIMOD_FOCAL_COMPONENTS__: JSON.stringify(docs?.components ?? []) } };
    },
    configureServer(server) {
      if (!directory) return;
      const source = directory;
      const allowed = new Set([...files].map((name) => resolve(source, name)));
      server.middlewares.use(staticFiles(source, `/${sdkDocsPath}/`, (path) => allowed.has(path)));
    },
    writeBundle(options) {
      if (this.environment.name !== 'client' || !directory || !options.dir) return;
      const destination = resolve(options.dir, sdkDocsPath);
      for (const name of files) {
        const target = resolve(destination, name);
        mkdirSync(dirname(target), { recursive: true });
        copyFileSync(resolve(directory, name), target);
      }
    },
  };
}
