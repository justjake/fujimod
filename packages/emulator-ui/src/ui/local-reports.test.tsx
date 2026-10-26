import { test, vi } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { LocalReports } from './local-reports';
import type { EngineFault } from '../engine/fault';

test('both sources save crash metadata before requesting a snapshot and deduplicate the same error', async () => {
  const marker = document.createElement('meta');
  marker.name = 'fujimod-local-reports';
  marker.content = '/__simulator-reports';
  document.head.append(marker);
  const requests: { url: string; body: unknown }[] = [];
  let captures = 0;
  const fetch = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, options) => {
    requests.push({
      url: url instanceof Request ? url.url : url instanceof URL ? url.href : url,
      body: options?.body,
    });
    return new Response('{}', { status: 201 });
  });
  try {
    for (const source of ['emulator', 'dev-tools'] as const) {
      const reports = new LocalReports(
        source,
        () => ({ event: 'feedback', modBuild: 'test', simulatorBuild: 'build' }),
        async () => {
          assert.equal(JSON.parse(String(requests.at(-1)!.body)).source, source);
          captures++;
          return new Blob(['ZIP']);
        },
      );
      const error = Error('worker stopped');
      reports.crash(error);
      reports.crash(error);
      await vi.waitFor(() =>
        assert.equal(
          requests.filter((request) => request.url.endsWith('report.zip')).length,
          captures,
        ),
      );
      await vi.waitFor(() => assert.equal(captures, source === 'emulator' ? 1 : 2));
    }
    assert.equal(requests.length, 4);
    marker.remove();
    const reports = new LocalReports(
      'dev-tools',
      () => ({ event: 'feedback', modBuild: 'test', simulatorBuild: 'build' }),
      async () => {
        throw Error('must not capture on published site');
      },
    );
    const faults: string[] = [];
    const announce = (event: Event) =>
      faults.push(String((event as CustomEvent<EngineFault>).detail.details.error));
    addEventListener('fujimod-fault', announce);
    reports.crash(Error('published site'));
    reports.crash(Error('uncaught on published site'), true);
    removeEventListener('fujimod-fault', announce);
    assert.equal(requests.length, 4);
    assert.deepEqual(faults, ['published site']);
  } finally {
    fetch.mockRestore();
    marker.remove();
  }
});
