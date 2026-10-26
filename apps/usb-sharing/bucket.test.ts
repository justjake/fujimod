import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';

await test('only bucket.ts writes the private bucket, so every object gets an index row', () => {
  const sources = readdirSync(new URL('.', import.meta.url)).filter(
    (name) => name.endsWith('.ts') && !name.endsWith('.test.ts') && name !== 'bucket.ts',
  );
  for (const name of sources) {
    const source = readFileSync(new URL(name, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /ISSUE_REPORTS\.put\(|createMultipartUpload\(|\.complete\(/, `${name} writes R2 directly`);
  }
});
