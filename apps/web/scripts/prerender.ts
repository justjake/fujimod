import { access, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Wizard, toolPath } from '../src/dev-tools/routes.ts';
const tools = Object.values(Wizard).map(toolPath);
export const routes = ['/', '/dev-tools/', ...tools, '/bug/', '/recovery/'];
/** Render through start mode's production handler, using the same entries as dev and hydration. */
export async function prerender(output = 'dist') {
  const { handleRequest } = await import(pathToFileURL(resolve(output, 'server/server.js')).href);
  for (const route of routes) {
    const response = await handleRequest(
      new Request(`https://prerender.invalid${route}`, { headers: { accept: 'text/html' } }),
      { renderMode: 'async' },
    );
    if (!response.ok) throw Error(`Prerender ${route}: HTTP ${response.status}`);
    const html = await response.text();
    const expected = tools.includes(route)
      ? 'Dev Tools'
      : {
          '/': 'id="workspace"',
          '/dev-tools/': 'Dev Tools',
          '/bug/': 'class="shell bug-report"',
          '/recovery/': 'class="shell recovery"',
        }[route];
    if (!html.includes(expected) || !/<script[^>]+type="module"/.test(html))
      throw Error(`Prerender ${route}: missing content or hydration entry`);
    for (const [, asset] of html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)) {
      try {
        const pathname = new URL(asset, 'https://prerender.invalid').pathname;
        await access(resolve(output, 'client', decodeURIComponent(pathname).slice(1)));
      } catch {
        throw Error(`Prerender ${route}: missing client asset ${asset}`);
      }
    }
    const directory = resolve(output, 'client', route.slice(1));
    await mkdir(directory, { recursive: true });
    await writeFile(
      resolve(directory, 'index.html'),
      '<!doctype html>' + html.replace(/^<!doctype html>/i, ''),
    );
    console.log(`Prerendered ${route}`);
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  await prerender(process.argv[2]);
