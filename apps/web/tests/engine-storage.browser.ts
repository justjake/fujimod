/** Real worker OPFS publication, revisions and failed transaction cleanup. */
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import type { StorageReply, StorageResult, ResumeResult } from './engine-storage.worker';
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const output = resolve('build/engine-storage');
await mkdir(output, { recursive: true });
for (const type of [chromium, webkit]) {
  const profile = await mkdtemp(join(output, type.name() + '-profile-'));
  const context = await type.launchPersistentContext(profile, { headless: true });
  try {
    await context.routeWebSocket('**/*', () => {});
    const page = await context.newPage();
    await page.goto(base);
    async function storageWorker(id?: string) {
      const reply = await page.evaluate(
        (id) =>
          new Promise<StorageReply>((resolve, reject) => {
            const worker = new Worker(new URL('/tests/engine-storage.worker.ts', location.origin), {
              type: 'module',
            });
            worker.onerror = (event) => {
              worker.terminate();
              reject(Error(event.message));
            };
            worker.onmessage = ({ data }: MessageEvent<StorageReply>) => {
              worker.terminate();
              resolve(data);
            };
            worker.postMessage({ id });
          }),
        id,
      );
      if ('error' in reply) throw Error(reply.error);
      return reply.result;
    }
    const result = (await storageWorker()) as StorageResult;
    await writeFile(join(output, type.name() + '-attempt.json'), JSON.stringify(result, null, 2));
    assert.equal(result.one.durable.backend, 'opfs');
    assert.equal(result.two.durable.backend, 'opfs');
    assert.notEqual(result.one.durable.file, result.two.durable.file);
    assert.deepEqual(result.retained, [4, 5, 6]);
    assert.deepEqual(result.bytes, [7, 8]);
    assert.match(result.stale, /another tab/);
    assert.match(result.quota, /quota/i);
    assert.deepEqual(result.after, result.two);
    assert.deepEqual(result.files, [result.two.durable.file]);
    await page.reload();
    const resumed = (await storageWorker(result.id)) as ResumeResult;
    assert.deepEqual(resumed.bytes, [7, 8]);
    assert.equal(resumed.record!.revision, 2);
    await writeFile(
      join(output, type.name() + '.json'),
      JSON.stringify({ result, resumed }, null, 2) + '\n',
    );
    console.log(
      `${type.name()}: worker OPFS, stale/quota preservation, cleanup and reload passed.`,
    );
  } finally {
    await context.close();
    await rm(profile, { recursive: true, force: true });
  }
}
