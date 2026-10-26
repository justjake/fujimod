// Run every declared one-shot integration suite; the ordinary test/coverage
// command runs separately. Watch commands are interactive developer tools.
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

const directory = resolve(process.argv[2]);
const { scripts } = JSON.parse(readFileSync(resolve(directory, 'package.json'), 'utf8'));
let failed = false;
for (const name of Object.keys(scripts)) {
  if (!name.startsWith('test:') || name.endsWith(':watch')) continue;
  const result = spawnSync('npm', ['run', name], { cwd: directory, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) failed = true;
}
process.exitCode = failed ? 1 : 0;
