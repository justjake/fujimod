import { readFileSync, existsSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join, resolve } from 'node:path';
import { cycloneDx } from './bom.ts';
import { cargoComponents } from './cargo.ts';
import { rustStandardLibrary } from './toolchain.ts';
import { npmComponents } from './npm.ts';
import { writeLicenseIndex } from './licenses.ts';
import { workerPackages } from './worker.ts';
import { readRuntimeSbom } from './runtime.ts';

const filename = '.sbom-npm-inputs.json';
const output = resolve(process.argv[2]);
const root = resolve(import.meta.dirname, '../../../..');
const client = join(output, 'client');
const clientReceipt = join(client, filename);
if (!existsSync(clientReceipt)) throw Error('Missing client bundle SBOM inputs');
const packages = JSON.parse(readFileSync(clientReceipt, 'utf8')) as string[];
const previewRuntime = process.argv.includes('--local-preview') && process.env.FUJIMOD_RUNTIME;
const runtime = previewRuntime || join(client, 'runtime');
const receipt = join(runtime, 'sbom-components.json');
const hasReceipt = existsSync(receipt);
if (!hasReceipt && (previewRuntime || existsSync(join(runtime, 'catalog.json'))))
  throw Error(`Missing SBOM provenance from runtime build: ${receipt}`);
const assets = hasReceipt ? readRuntimeSbom(runtime, receipt) : undefined;
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
