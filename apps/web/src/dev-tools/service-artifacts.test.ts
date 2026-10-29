import { afterEach, expect, test, vi } from 'vite-plus/test';
import { serviceWorkflow } from './service-artifacts';
import { debugModeOperations, type DebugModeSession } from './debug-mode/operations';

const firmware = {
  id: 'xt1-552',
  body: 'xt1',
  model: 'X-T1',
  version: '5.52',
  sha256: 'a'.repeat(64),
  bytes: 42,
  download_url: 'https://example.org/update',
  release_date: '',
  service_workflow: 'runtime/xt1-552/service-workflow.generated.json',
};
function catalog(
  entry: Omit<typeof firmware, 'service_workflow'> & { service_workflow?: string } = firmware,
) {
  vi.stubGlobal('document', { baseURI: 'https://example.org/' });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: URL) =>
      url.pathname.endsWith('service-workflow.generated.json')
        ? new Response('{"schema":1,"setup":{},"persistence":{}}')
        : Response.json({ schema: 1, firmwares: [entry] }),
    ),
  );
}
afterEach(() => vi.unstubAllGlobals());

test('physical-camera workflow resolves without an emulator bundle', async () => {
  catalog();
  expect(await serviceWorkflow('xt1-552')).toEqual([
    '{"schema":1,"setup":{},"persistence":{}}',
    'xt1-552',
    'a'.repeat(64),
  ]);
});

test('an unavailable workflow reports the selected model and release', async () => {
  catalog({ ...firmware, service_workflow: undefined });
  await expect(serviceWorkflow('xt1-552')).rejects.toThrow(
    'Service setup is not supported for X-T1 5.52.',
  );
  expect(fetch).toHaveBeenCalledTimes(1);
});

test('both debug actions use the generated definition before opening their session', async () => {
  catalog();
  const enter = vi.fn<DebugModeSession['enterDebugMode']>(async () => ({
    alreadyPersisted: false,
    progress: ['enabled'],
  }));
  const leave = vi.fn<DebugModeSession['leaveDebugMode']>(async () => ({
    alreadyPersisted: true,
    progress: ['disabled'],
  }));
  const debug: DebugModeSession = {
    armResetGuard: vi.fn(),
    enterDebugMode: enter,
    leaveDebugMode: leave,
  };
  const operations = debugModeOperations(async (_tool, operation) => operation(debug));
  const progress = vi.fn();
  await operations.enter('xt1-552', progress);
  await operations.leave('xt1-552', progress);
  for (const action of [enter, leave])
    expect(action).toHaveBeenCalledExactlyOnceWith(
      '{"schema":1,"setup":{},"persistence":{}}',
      'xt1-552',
      'a'.repeat(64),
    );
  expect(progress.mock.calls).toEqual([['enabled'], ['disabled']]);
});

test('setup remains available when persistence cannot be located', async () => {
  catalog();
  const fetcher = vi.mocked(fetch);
  fetcher.mockImplementationOnce(async () => Response.json({ schema: 1, firmwares: [firmware] }));
  fetcher.mockImplementationOnce(async () =>
    Response.json({ schema: 1, setup: {}, persistence: null }),
  );
  await expect(serviceWorkflow('xt1-552')).resolves.toEqual([
    JSON.stringify({ schema: 1, setup: {}, persistence: null }),
    'xt1-552',
    'a'.repeat(64),
  ]);
  fetcher.mockImplementationOnce(async () => Response.json({ schema: 1, firmwares: [firmware] }));
  fetcher.mockImplementationOnce(async () =>
    Response.json({ schema: 1, setup: {}, persistence: null }),
  );
  const run = vi.fn();
  await expect(debugModeOperations(run).enter('xt1-552', vi.fn())).rejects.toThrow(
    'Persistent debug mode is not supported for X-T1 5.52.',
  );
  expect(run).not.toHaveBeenCalled();
});
