import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

await test('package runner keeps failures, runs later suites, and excludes interactive watch commands', (t) => {
  const output = fileURLToPath(new URL('../../target/ci/', import.meta.url));
  mkdirSync(output, { recursive: true });
  const directory = mkdtempSync(`${output}package-tests-`);
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  writeFileSync(
    `${directory}/package.json`,
    JSON.stringify({
      scripts: {
        'test:first': 'node -e "process.exit(1)"',
        'test:second': "node -e \"require('node:fs').writeFileSync('second', 'ok')\"",
        'test:watch': "node -e \"require('node:fs').writeFileSync('watch', 'bad')\"",
      },
    }),
  );
  const result = spawnSync('node', [
    fileURLToPath(new URL('./package-tests.ts', import.meta.url)),
    directory,
  ]);
  assert.equal(result.status, 1, result.stderr?.toString());
  assert.ok(existsSync(`${directory}/second`));
  assert.ok(!existsSync(`${directory}/watch`));
});
