import { readFileSync, existsSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join, resolve } from 'node:path';
import { cycloneDx } from './bom.ts';
import { cargoComponents } from './cargo.ts';
import { rustStandardLibrary } from './toolchain.ts';
import { npmComponents } from './npm.ts';
import { writeLicenseIndex } from './licenses.ts';
import { workerPackages } from './worker.ts';
import type { RuntimeSbom } from './runtime.ts';

const filename = '.sbom-npm-inputs.json';
const output = resolve(process.argv[2]);
const root = resolve(import.meta.dirname, '../../../..');
const client = join(output, 'client');
const clientReceipt = join(client, filename);
if (!existsSync(clientReceipt)) throw Error('Missing client bundle SBOM inputs');
const packages = JSON.parse(readFileSync(clientReceipt, 'utf8')) as string[];
const runtime = process.env.FUJIMOD_RUNTIME ?? resolve('public/runtime');
const receipt = join(runtime, 'sbom-components.json');
const hasReceipt = existsSync(receipt);
if (!hasReceipt && (process.env.FUJIMOD_RUNTIME || existsSync(join(runtime, 'catalog.json'))))
  throw Error(`Missing SBOM provenance from runtime build: ${receipt}`);
const assets: RuntimeSbom | undefined = hasReceipt
  ? JSON.parse(readFileSync(receipt, 'utf8'))
  : undefined;
if (assets && (assets.schema !== 2 || !Array.isArray(assets.components)))
  throw Error('Runtime SBOM predates linker provenance. Stage a freshly built runtime.');
const linkedDirectories: string[] = JSON.parse(
  readFileSync(join(root, 'bindings/javascript/.tools/webusb/linked-crates.json'), 'utf8'),
);
const bom = cycloneDx([
  ...npmComponents(root, [...packages, ...workerPackages(root, output)]),
  ...cargoComponents(root, [
    {
      package: 'fujimod-javascript',
      target: 'wasm32-unknown-unknown',
      product: 'website',
      linkedDirectories,
    },
  ]),
  rustStandardLibrary(root, 'website'),
  ...(assets?.components ?? []),
]);
writeLicenseIndex(bom, output);
const destination = join(client, 'sbom.json');
const temporary = destination + '.' + randomUUID() + '.tmp';
try {
  writeFileSync(temporary, JSON.stringify(bom, null, 2) + '\n');
  renameSync(temporary, destination);
} finally {
  rmSync(temporary, { force: true });
}
rmSync(clientReceipt);
console.log(`Wrote ${destination} (${bom.components.length} components)`);
