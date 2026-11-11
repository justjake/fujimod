import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';

/** A Google Photos endpoint that records what a camera sent it. */
export interface FakePhotos {
  baseUrl: string;
  uploads: Uint8Array[];
  created: { fileName: string; uploadToken: string; authorization: string }[];
  close(): Promise<void>;
}

async function readBody(stream: NodeJS.ReadableStream): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(Buffer.from(chunk as Uint8Array));
  return Buffer.concat(chunks);
}

export async function startFakePhotos(): Promise<FakePhotos> {
  const uploads: Uint8Array[] = [];
  const created: FakePhotos['created'] = [];
  const server = createServer((request, response) => {
    void (async () => {
      const path = request.url ?? '';
      const authorization = request.headers.authorization ?? '';
      if (request.method === 'POST' && path === '/v1/uploads') {
        uploads.push(new Uint8Array(await readBody(request)));
        response.writeHead(200, { 'content-type': 'text/plain' });
        response.end(`upload-${uploads.length}`);
        return;
      }
      if (request.method === 'POST' && path === '/v1/mediaItems:batchCreate') {
        const body = JSON.parse((await readBody(request)).toString('utf8')) as {
          newMediaItems: { simpleMediaItem: { fileName: string; uploadToken: string } }[];
        };
        const item = body.newMediaItems[0]!.simpleMediaItem;
        created.push({ ...item, authorization });
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(
          JSON.stringify({
            newMediaItemResults: [{ mediaItem: { id: `media-item-${created.length}` } }],
          }),
        );
        return;
      }
      response.writeHead(404);
      response.end();
    })().catch((error: unknown) => {
      response.writeHead(500);
      response.end(String(error));
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return {
    baseUrl: `http://127.0.0.1:${port}`,
    uploads,
    created,
    close: async () => {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
