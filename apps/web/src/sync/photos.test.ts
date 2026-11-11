import { expect, test } from 'vite-plus/test';
import { PhotosUploadError, uploadPhoto } from './photos.ts';

interface Call {
  url: string;
  init: RequestInit;
}

function scripted(...responses: Response[]) {
  const calls: Call[] = [];
  const fetch: typeof globalThis.fetch = async (
    input: string | URL | Request,
    init?: RequestInit,
  ) => {
    calls.push({ url: String(input), init: init ?? {} });
    const response = responses.shift();
    if (!response) throw new Error('The client sent an unexpected request.');
    return response;
  };
  return { calls, fetch };
}

test('a photo uploads raw bytes and is attached by its upload token', async () => {
  const { calls, fetch } = scripted(
    new Response('upload-token-1', { status: 200 }),
    Response.json({ newMediaItemResults: [{ mediaItem: { id: 'item-1' } }] }),
  );
  const bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);

  const uploaded = await uploadPhoto(
    { fileName: 'DSCF0001.PNG', bytes, contentType: 'image/png' },
    { accessToken: 'google-token', baseUrl: 'https://photos.example', fetch },
  );

  expect(uploaded).toEqual({ fileName: 'DSCF0001.PNG', mediaItemId: 'item-1' });
  expect(calls.map((call) => call.url)).toEqual([
    'https://photos.example/v1/uploads',
    'https://photos.example/v1/mediaItems:batchCreate',
  ]);
  const upload = calls[0]!;
  const headers = new Headers(upload.init.headers);
  expect(headers.get('authorization')).toBe('Bearer google-token');
  expect(headers.get('content-type')).toBe('application/octet-stream');
  expect(headers.get('x-goog-upload-protocol')).toBe('raw');
  expect(headers.get('x-goog-upload-content-type')).toBe('image/png');
  expect([...(upload.init.body as Uint8Array)]).toEqual([...bytes]);
  expect(JSON.parse(String(calls[1]!.init.body))).toEqual({
    newMediaItems: [
      { simpleMediaItem: { fileName: 'DSCF0001.PNG', uploadToken: 'upload-token-1' } },
    ],
  });
});

test("a refused upload reports Google's message", async () => {
  const { fetch } = scripted(
    Response.json({ error: { message: 'Invalid upload content' } }, { status: 400 }),
  );
  const failure = uploadPhoto(
    { fileName: 'bad.jpg', bytes: new Uint8Array([1]) },
    { accessToken: 'google-token', baseUrl: 'https://photos.example', fetch },
  );
  await expect(failure).rejects.toThrow('Invalid upload content');
  await expect(failure).rejects.toBeInstanceOf(PhotosUploadError);
});

test('a per-item rejection fails the photo', async () => {
  const { fetch } = scripted(
    new Response('upload-token-1', { status: 200 }),
    Response.json({
      newMediaItemResults: [{ status: { code: 3, message: 'File type is unsupported' } }],
    }),
  );
  const failure = uploadPhoto(
    { fileName: 'bad.tiff', bytes: new Uint8Array([1]) },
    { accessToken: 'google-token', baseUrl: 'https://photos.example', fetch },
  );
  await expect(failure).rejects.toThrow('File type is unsupported');
});

test('an empty upload token is refused before creating the item', async () => {
  const { calls, fetch } = scripted(new Response('  ', { status: 200 }));
  const failure = uploadPhoto(
    { fileName: 'photo.jpg', bytes: new Uint8Array([1]) },
    { accessToken: 'google-token', baseUrl: 'https://photos.example', fetch },
  );
  await expect(failure).rejects.toThrow('empty upload token');
  expect(calls).toHaveLength(1);
});
