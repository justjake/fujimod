import { mkdir, rm, writeFile } from 'node:fs/promises';

// Generated reference material stays out of the application's source and assets.
const output = new URL('../build/docs/solidjs/', import.meta.url);
const pages = ['llms.txt', 'llms-small.txt', 'llms-full.txt'];
const docs = await Promise.all(
  pages.map(async (page) => {
    const url = new URL(page, 'https://v2.solidjs.com/');
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return [page, await response.text()];
  }),
);
// Replace the generated cache so guides from an older major cannot linger.
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
for (const [page, text] of docs) await writeFile(new URL(page, output), text);
console.log(`Official SolidJS 2 docs downloaded to ${output.pathname}`);
