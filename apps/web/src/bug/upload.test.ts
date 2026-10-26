import { test, expect, vi } from 'vite-plus/test';
import { uploadReport } from './upload';
import { link, tar } from './fixtures.test-helper';
test('explicit upload omits inline descriptor bytes and validates the acknowledgment before deletion is possible', async () => {
  const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
  const fetcher = vi.fn<typeof fetch>(async () =>
    Response.json({ id, issueURL: 'https://github.com/justjake/fujimod/issues/123' }),
  );
  vi.stubGlobal('fetch', fetcher);
  try {
    await uploadReport(new Blob([tar]), { ...link, archive: 'unused' }, 'Frozen menu', id);
    const init = fetcher.mock.calls[0]?.[1] as RequestInit;
    const body = init.body as FormData;
    expect(JSON.parse(String(body.get('metadata'))).archive).toBeUndefined();
    expect(body.get('description')).toBe('Frozen menu');
    fetcher.mockResolvedValueOnce(Response.json({ id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' }));
    await expect(uploadReport(new Blob([tar]), link, '', id)).rejects.toThrow('did not confirm');
    fetcher.mockResolvedValueOnce(new Response('', { status: 503 }));
    await expect(uploadReport(new Blob([tar]), link, '', id)).rejects.toThrow('not configured');
  } finally {
    vi.unstubAllGlobals();
  }
});
