import { afterEach, expect, test, vi } from 'vite-plus/test';
import { createHash, randomFillSync } from 'node:crypto';
import { gunzipSync } from 'node:zlib';
import { contribute, type ContributionProgress } from './contribute';
const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
function urlText(value: Parameters<typeof fetch>[0]) {
  return value instanceof Request ? value.url : value.toString();
}
const failure = 'Upload failed. Your files are still on this device. Try again.';
afterEach(() => vi.unstubAllGlobals());
function server(blobs: Blob[]) {
  const files = blobs.map(() => ({
    complete: false,
    parts: [] as { partNumber: number; bytes: number }[],
  }));
  const uploaded = blobs.map(() => new Map<number, Uint8Array>());
  let finished = false;
  const fetcher = vi.fn<typeof fetch>(async (url, init) => {
    expect(init?.credentials).toBe('omit');
    const path = urlText(url).split(`/contributions/${id}`)[1];
    if (path === '') return Response.json({ id, files, finished });
    if (path === '/finish') {
      expect(files.every((file) => file.complete)).toBe(true);
      finished = true;
      return Response.json({ id });
    }
    const match = /^\/files\/(\d+)(?:\/(parts\/(\d+)|complete))?$/.exec(path)!;
    const index = Number(match[1]),
      file = files[index];
    if (match[3]) {
      const partNumber = Number(match[3]),
        data = new Uint8Array(await (init!.body as Blob).arrayBuffer());
      uploaded[index].set(partNumber, data);
      file.parts.push({ partNumber, bytes: data.length });
      return Response.json({ partNumber });
    }
    if (match[2] === 'complete') {
      expect(JSON.parse(init!.body as string).parts).toBe(file.parts.length);
      file.complete = true;
    }
    return Response.json(file);
  });
  vi.stubGlobal('fetch', fetcher);
  return { fetcher, files, uploaded };
}
test('streams gzip in 8 MiB parts, reports uncompressed progress, and resumes saved parts', async () => {
  const bytes = randomFillSync(new Uint8Array(18 * 1024 * 1024));
  const blob = new Blob([bytes]),
    empty = new Blob([]),
    fake = server([blob, empty]);
  const realFetch = fake.fetcher.getMockImplementation()!;
  let fail = true;
  fake.fetcher.mockImplementation(async (url, init) => {
    if (urlText(url).endsWith('/parts/2') && fail) throw Error('Offline');
    return realFetch(url, init);
  });
  const input = {
    id,
    files: [blob, empty].map((blob, i) => ({
      model: 'X70',
      serial: '123',
      name: `${i}.bin`,
      blob,
    })),
    manifest: { version: 1 },
    message: 'Capture',
  };
  const progress: ContributionProgress[] = [];
  await expect(contribute(input, (p) => progress.push(p))).rejects.toThrow(failure);
  expect(fake.files[0].parts.map((part) => part.partNumber)).toEqual([1]);
  expect(fake.fetcher.mock.calls.filter(([url]) => urlText(url).endsWith('/parts/2'))).toHaveLength(
    4,
  );
  fail = false;
  fake.fetcher.mockClear();
  progress.length = 0;
  expect(await contribute(input, (p) => progress.push(p))).toEqual({ id });
  expect(fake.fetcher.mock.calls.some(([url]) => urlText(url).endsWith('/files/0/parts/1'))).toBe(
    false,
  );
  const parts = [...fake.uploaded[0].values()];
  expect(parts[0].length).toBe(8 * 1024 * 1024);
  expect(parts[1].length).toBe(8 * 1024 * 1024);
  expect(
    createHash('sha256')
      .update(gunzipSync(Buffer.concat(parts)))
      .digest('hex'),
  ).toBe(createHash('sha256').update(bytes).digest('hex'));
  expect(gunzipSync(Buffer.concat([...fake.uploaded[1].values()])).length).toBe(0);
  expect(progress[0]).toEqual({ done: 0, total: bytes.length });
  expect(progress.at(-1)).toEqual({ done: bytes.length, total: bytes.length });
  expect(
    progress.every((p, i) => p.done <= p.total && (i === 0 || p.done >= progress[i - 1].done)),
  ).toBe(true);
  fake.fetcher.mockClear();
  await contribute(input, () => {});
  expect(fake.fetcher).toHaveBeenCalledTimes(1);
});
test('a completed file is skipped when finish must be retried', async () => {
  const blob = new Blob(['camera bytes']),
    fake = server([blob]);
  const input = {
    id,
    files: [{ model: 'X70', serial: '123', name: 'flash.bin', blob }],
    manifest: {},
    message: '',
  };
  const original = fake.fetcher.getMockImplementation()!;
  fake.fetcher.mockImplementation(async (url, init) =>
    urlText(url).endsWith('/finish') ? new Response('', { status: 503 }) : original(url, init),
  );
  await expect(contribute(input, () => {})).rejects.toThrow(failure);
  fake.fetcher.mockImplementation(original).mockClear();
  await contribute(input, () => {});
  expect(fake.fetcher.mock.calls.map(([url]) => urlText(url).split(id)[1])).toEqual([
    '',
    '/finish',
  ]);
});
test('aborts stop requests and malformed receipts or network errors show the retry message', async () => {
  const blob = new Blob(['data']),
    fake = server([blob]);
  const input = {
    id,
    files: [{ model: 'X70', serial: '1', name: 'flash.bin', blob }],
    manifest: {},
    message: '',
  };
  const controller = new AbortController();
  controller.abort();
  await expect(contribute(input, () => {}, controller.signal)).rejects.toMatchObject({
    name: 'AbortError',
  });
  expect(fake.fetcher).not.toHaveBeenCalled();
  fake.fetcher.mockResolvedValueOnce(Response.json({ id: 'wrong', files: [] }));
  await expect(contribute(input, () => {})).rejects.toThrow(failure);
  fake.fetcher.mockRejectedValueOnce(Error('Network details'));
  await expect(contribute(input, () => {})).rejects.toThrow(failure);
  const mid = new AbortController();
  fake.fetcher.mockClear();
  await expect(
    contribute(
      input,
      (p) => {
        if (p.done) mid.abort();
      },
      mid.signal,
    ),
  ).rejects.toMatchObject({ name: 'AbortError' });
  expect(fake.fetcher.mock.calls.some(([url]) => urlText(url).includes('/parts/'))).toBe(false);
});
