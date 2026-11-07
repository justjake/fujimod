/** Write camera-labels.generated.json from the catalog; `--check` fails when it is stale. */
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { cameraLabels } from '../camera-labels.ts';

const root = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const output = root('../camera-labels.generated.json');
const names = JSON.parse(await readFile(root('../processor-names.json'), 'utf8')) as Record<
  string,
  string
>;
const text = JSON.stringify(await cameraLabels(root('../../../catalog'), names), null, 2) + '\n';
if (process.argv.includes('--check')) {
  if ((await readFile(output, 'utf8').catch(() => '')) !== text)
    throw Error('camera-labels.generated.json is stale; run pnpm run generate');
} else await writeFile(output, text);
