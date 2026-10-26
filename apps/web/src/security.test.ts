import { test } from 'vitest';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { contentSecurityPolicy, runtimeHeaders } from './security';
import middleware from './middleware';
test('SSR and static hosting enforce the same Wasm policy', async () => {
  const response = await middleware(
    { request: new Request('https://example.invalid'), locals: {} },
    async () => new Response(''),
  );
  for (const [name, value] of Object.entries(runtimeHeaders))
    assert.equal(response.headers.get(name), value);
  const headers = await readFile(new URL('../public/_headers', import.meta.url), 'utf8');
  assert.equal(headers.trim(), '/*\n' + Object.entries(runtimeHeaders).map(([name, value]) => `  ${name}: ${value}`).join('\n'));
  assert.ok(!contentSecurityPolicy.includes("'unsafe-eval'"));
});
