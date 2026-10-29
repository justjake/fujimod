import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { validateIndex, verifyPatch } from '../src/patch/catalog.ts';
const path = process.argv[2],
  index = validateIndex(JSON.parse(await readFile(path, 'utf8')));
for (const entry of index.firmwares)
  await verifyPatch(
    index,
    entry,
    new Uint8Array(await readFile(join(dirname(path), index.patches[entry.sha256]))),
  );
console.log(`Release index and ${index.firmwares.length} patch(es) verified: ${index.release}`);
