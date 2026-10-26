import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const compiler = join(packageRoot, 'node_modules/typescript/bin/tsc');
const [sourceDirectory, outputDirectory] = process.argv.slice(2);
if (!sourceDirectory || !outputDirectory)
  throw new Error('JIT adapter build requires source and output directories.');

execFileSync(
  process.execPath,
  ['--disable-warning=ExperimentalWarning', compiler, '-p', join(sourceDirectory, 'tsconfig.json')],
  {
    stdio: 'inherit',
  },
);
mkdirSync(outputDirectory, { recursive: true });
for (const name of ['host']) {
  const source = join(sourceDirectory, `${name}.ts`);
  const javascript = stripTypeScriptTypes(readFileSync(source, 'utf8'), {
    mode: 'strip',
    sourceUrl: `${name}.ts`,
  });
  writeFileSync(join(outputDirectory, `${name}.js`), javascript);
}
