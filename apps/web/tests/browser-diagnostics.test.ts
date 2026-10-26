import type { ViteDevServer, Connect } from 'vite';
import { browserScript } from '../scripts/browser-script.ts';
import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runInNewContext } from 'node:vm';
import { browserDiagnostics } from '../scripts/browser-diagnostics.ts';

test('development diagnostics capture async failures, console errors and LCD state without URL credentials', async () => {
  const sent: {
    kind: string;
    detail?: string;
    frames?: string;
    webgpu?: boolean;
    emulatorBootStage?: string;
    emulatorMemoryBytes?: number;
    emulatorPeakMemoryBytes?: number;
  }[] = [];
  const events: Record<
    string,
    (event: { reason?: string; message?: string; detail?: Record<string, unknown> }) => void
  > = {};
  const timers: (() => void)[] = [];
  const browserConsole = { error(..._args: unknown[]) {}, warn() {} };
  const source = await browserScript(
    new URL('../scripts/browser-diagnostics-client.ts', import.meta.url),
  );
  runInNewContext(source, {
    crypto: { randomUUID: () => 'test-session' },
    console: browserConsole,
    navigator: { userAgent: 'test Safari', gpu: {} },
    location: { pathname: '/' },
    performance: { now: () => 5000, getEntriesByType: () => [] },
    document: {
      visibilityState: 'visible',
      addEventListener() {},
      querySelector: () => ({
        dataset: { renderer: 'lcd', frames: '2' },
      }),
    },
    window: {
      addEventListener: (type: string, callback: (typeof events)[string]) => {
        events[type] = callback;
      },
    },
    fetch: async (_url: string, request: { body: string }) => sent.push(JSON.parse(request.body)),
    setTimeout: (callback: () => void) => timers.push(callback),
  });
  browserConsole.error('LCD display failure');
  events.unhandledrejection({
    reason: 'GPU failure https://example.com/file?token=secret#fragment',
  });
  events.error({ message: 'shader validation failed' });
  timers[0]();
  assert.deepEqual(
    sent.map((entry) => entry.kind),
    ['startup', 'console.error', 'unhandledrejection', 'error', 'checkpoint'],
  );
  assert.equal(sent[2].detail, 'GPU failure https://example.com/file');
  assert.equal(sent[4].frames, '2');
  assert.equal(sent[4].webgpu, true);
  events['camera-boot-memory']({
    detail: { stage: 'board-created', wasmMemoryBytes: 600000000, peakWasmMemoryBytes: 700000000 },
  });
  assert.equal(sent[5].kind, 'emulator-memory');
  assert.equal(sent[5].emulatorBootStage, 'board-created');
  assert.equal(sent[5].emulatorMemoryBytes, 600000000);
  assert.equal(sent[5].emulatorPeakMemoryBytes, 700000000);
  events['simulator-download']({ detail: { name: 'simulator-report.zip', bytes: 1234 } });
  assert.equal(sent[6].kind, 'download');
  assert.equal(sent[6].detail, 'simulator-report.zip (1234 bytes)');
});

test('diagnostic endpoint saves valid reports and rejects oversized, malformed and cross-origin requests', async () => {
  const root = await mkdtemp(join(tmpdir(), 'fujimod-browser-'));
  const logs: string[] = [];
  let middleware: Connect.NextHandleFunction | undefined;
  const plugin = browserDiagnostics();
  assert.equal(plugin.apply, 'serve');
  const configure = plugin.configureServer;
  assert.ok(typeof configure === 'function');
  await configure.call(
    {} as ThisParameterType<typeof configure>,
    {
      config: { root, logger: { info: (line: string) => logs.push(line) } },
      middlewares: {
        use: (handler: Connect.NextHandleFunction) => {
          middleware = handler;
        },
      },
    } as unknown as ViteDevServer,
  );
  const server = createServer((req, res) =>
    middleware!(req, res, () => {
      res.statusCode = 404;
      res.end();
    }),
  );
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  const url = `http://127.0.0.1:${address.port}/__browser-diagnostics`;
  const post = (body: string, headers: Record<string, string> = {}) =>
    fetch(url, { method: 'POST', body, headers });
  try {
    assert.equal(
      (await post(JSON.stringify({ session: 'phone', kind: 'console.error', detail: 'GPU panic' })))
        .status,
      204,
    );
    assert.equal((await post('not json')).status, 400);
    assert.equal((await post('x'.repeat(32769))).status, 413);
    assert.equal((await post('{}', { Origin: 'https://other.example' })).status, 403);
    assert.equal((await post('{}', { Origin: 'invalid' })).status, 403);
    const report = JSON.parse(
      await readFile(join(root, 'build/web-service/browser-diagnostics.jsonl'), 'utf8'),
    );
    assert.equal(report.detail, 'GPU panic');
    assert.equal(logs.length, 1);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(root, { recursive: true });
  }
});
