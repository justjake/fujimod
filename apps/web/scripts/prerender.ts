import { access, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Wizard, toolPath } from '../src/dev-tools/routes.ts';
import { readSdkDocs, sdkDocsPath } from './sdk-docs.ts';
import { manualRoutes } from '../src/docs/manual-data.ts';
import { isManualPath } from '../src/docs/manual-routes.ts';
import { textPagePath, textPages } from '../src/text/routes.ts';
const tools = Object.values(Wizard).map(toolPath);
export function siteRoutes(client: string) {
  const components = readSdkDocs(resolve(client, sdkDocsPath))?.components ?? [];
  return [
    '/',
    '/dev-tools/',
    ...tools,
    '/bug/',
    '/recovery/',
    '/device/',
    ...Object.keys(textPages),
    ...manualRoutes(),
    '/docs/',
    '/docs/design/',
    '/docs/design/custom-settings-editor/',
    `/${sdkDocsPath}/`,
    ...components.map((component) => `/${sdkDocsPath}/components/${component.slug}/`),
  ];
}
/** Render through start mode's production handler, using the same entries as dev and hydration. */
export async function prerender(output = 'dist') {
  process.env.FUJIMOD_PRERENDER_OUTPUT = resolve(output);
  const { handleRequest } = await import(pathToFileURL(resolve(output, 'server/server.js')).href);
  for (const route of siteRoutes(resolve(output, 'client'))) {
    const response = await handleRequest(
      new Request(`https://prerender.invalid${route}`, { headers: { accept: 'text/html' } }),
      { renderMode: 'async' },
    );
    if (!response.ok) throw Error(`Prerender ${route}: HTTP ${response.status}`);
    const html = await response.text();
    const expected = textPagePath(route)
      ? 'class="text-page'
      : isManualPath(route)
        ? 'class="shell manual"'
        : route.startsWith('/docs/sdk/focal/')
          ? 'class="shell sdk-docs"'
          : tools.includes(route)
            ? 'Dev Tools'
            : {
                '/': 'id="workspace"',
                '/dev-tools/': 'Dev Tools',
                '/bug/': 'class="shell bug-report"',
                '/recovery/': 'class="shell recovery"',
                '/device/': 'class="shell device"',
                '/docs/': 'class="shell docs-overview"',
                '/docs/design/': 'class="shell design-index"',
                '/docs/design/custom-settings-editor/': 'class="shell design-page"',
              }[route];
    if (
      !html.includes('class="site-header"') ||
      !html.includes(expected) ||
      !/<script[^>]+type="module"/.test(html)
    )
      throw Error(`Prerender ${route}: missing content or hydration entry`);
    for (const [, asset] of html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)) {
      try {
        const pathname = new URL(asset, 'https://prerender.invalid').pathname;
        await access(resolve(output, 'client', decodeURIComponent(pathname).slice(1)));
      } catch {
        throw Error(`Prerender ${route}: missing client asset ${asset}`);
      }
    }
    const directory = resolve(output, 'client', route.endsWith('/') ? route.slice(1) : '');
    await mkdir(directory, { recursive: true });
    await writeFile(
      resolve(directory, route.endsWith('/') ? 'index.html' : route.slice(1) + '.html'),
      '<!doctype html>' + html.replace(/^<!doctype html>/i, ''),
    );
    console.log(`Prerendered ${route}`);
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  await prerender(process.argv[2]);
