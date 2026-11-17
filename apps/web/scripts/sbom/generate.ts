import { readFileSync, existsSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join, resolve } from 'node:path';
import { cycloneDx, type Component } from './bom.ts';
import { cargoComponents } from './cargo.ts';
import { rustStandardLibrary } from './toolchain.ts';
import { npmComponents } from './npm.ts';

const filename = '.sbom-npm-inputs.json';
const output = resolve(process.argv[2]);
const root = resolve(import.meta.dirname, '../../../..');
const client = join(output, 'client');
const receipts = ['client', 'server']
  .map((environment) => join(output, environment, filename))
  .filter((path) => existsSync(path));
if (!receipts.includes(join(client, filename))) throw Error('Missing client bundle SBOM inputs');
const packages = receipts.flatMap((path) => JSON.parse(readFileSync(path, 'utf8')) as string[]);
const runtime = process.env.FUJIMOD_RUNTIME ?? resolve('public/runtime');
const receipt = join(runtime, 'sbom-components.json');
const assets: Component[] = existsSync(receipt)
  ? (JSON.parse(readFileSync(receipt, 'utf8')) as Component[])
  : [];
if (process.env.FUJIMOD_RUNTIME && !existsSync(receipt))
  throw Error(`Missing SBOM provenance from runtime build: ${receipt}`);
const bom = cycloneDx([
  ...npmComponents(root, packages),
  ...cargoComponents(root, [
    { package: 'fujimod-javascript', target: 'wasm32-unknown-unknown', product: 'website' },
  ]),
  rustStandardLibrary(root, 'website'),
  ...assets,
]);
const destination = join(client, 'sbom.json');
const temporary = destination + '.' + randomUUID() + '.tmp';
try {
  writeFileSync(temporary, JSON.stringify(bom, null, 2) + '\n');
  renameSync(temporary, destination);
} finally {
  rmSync(temporary, { force: true });
}
for (const path of receipts) rmSync(path);
console.log(`Wrote ${destination} (${bom.components.length} components)`);
