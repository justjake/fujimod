import { expect, test } from 'vite-plus/test';
import { unzipSync } from 'fflate';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { serveOta, serveRelease, type ReleaseCache, type ReleaseStore } from '../worker';
import { loadRelease, type LoadedRelease } from '../src/patch/release';
import { sha256 } from '../src/patch/patcher';
import { loadOtaRelease, storeName } from '../src/ota/release';
import { sdBootV2Plan } from '../src/home/sd-boot-v2/plan';
import { buildCard } from '../src/home/sd-boot-v2/build';

const origin = 'https://fujimod.test';
const encode = (text: string) => new TextEncoder().encode(text);
type AssetReader = (tag: string, name: string) => Promise<Uint8Array | undefined>;

/** Real Worker handlers, with only GitHub transport and Cloudflare storage replaced. */
function releaseServer(readAsset: AssetReader) {
  const cached = new Map<string, Response>();
  const stored = new Map<string, Uint8Array>();
  const cache: ReleaseCache = {
    match: async (request) => cached.get(request.url)?.clone(),
    put: async (request, response) => {
      cached.set(request.url, response.clone());
    },
  };
  const store: ReleaseStore = {
    get: async (key) => stored.get(key)?.slice().buffer ?? null,
    put: async (key, value) => {
      stored.set(key, value.slice());
    },
  };
  const server = {
    current: 'A',
    publicPaths: [] as string[],
    upstreamPaths: [] as string[],
    cold() {
      cached.clear();
      stored.clear();
    },
    transport: (async (input) => {
      const url = new URL(input instanceof Request ? input.url : input);
      server.upstreamPaths.push(url.pathname);
      if (url.href.startsWith('https://api.github.com/repos/justjake/fujimod/releases?'))
        return Response.json(
          (server.current === 'B' ? ['A', 'B'] : [server.current]).map((tag, index) => ({
            tag_name: tag,
            name: tag,
            draft: false,
            prerelease: true,
            published_at: `2026-11-13T00:00:0${index}Z`,
            assets: [{ name: 'patches.json', state: 'uploaded' }],
          })),
        );
      const match = /^\/justjake\/fujimod\/releases\/download\/([^/]+)\/([^/]+)$/.exec(
        url.pathname,
      );
      if (url.origin !== 'https://github.com' || !match)
        throw Error(`Unexpected upstream request: ${url}`);
      const bytes = await readAsset(decodeURIComponent(match[1]), decodeURIComponent(match[2]));
      return bytes
        ? new Response(bytes.slice(), {
            headers: {
              'content-type': match[2].endsWith('.json')
                ? 'application/json'
                : 'application/octet-stream',
            },
          })
        : new Response(null, { status: 404 });
    }) as typeof fetch,
    fetch: (async (input, init) => {
      const request = new Request(input instanceof Request ? input : input.toString(), init);
      const path = new URL(request.url).pathname;
      server.publicPaths.push(path);
      const pending: Promise<unknown>[] = [];
      const context = {
        waitUntil: (promise: Promise<unknown>) => {
          pending.push(promise);
        },
      };
      const response = await (path.startsWith('/ota/') ? serveOta : serveRelease)(
        request,
        cache,
        store,
        context,
        server.transport,
      );
      await Promise.all(pending);
      return response;
    }) as typeof fetch,
  };
  return server;
}

async function fixture(tag: string) {
  const payload = encode(`payload ${tag}`),
    hash = await sha256(payload);
  const assets = new Map<string, Uint8Array>([[storeName(hash), payload]]);
  const card = await Promise.all(
    ['FFBOOT.BIN', 'FFSYS.BIN', 'FM_EXR2.BIN'].map(async (path) => {
      const bytes = encode(`${path} ${tag}`);
      assets.set(path, bytes);
      return {
        path,
        asset: path,
        sha256: await sha256(bytes),
        bytes: bytes.length,
        processor: 'EXR Processor II',
      };
    }),
  );
  assets.set(
    'X70-110-latest.json',
    encode(
      JSON.stringify({
        firmware: 'X70-110',
        version: tag,
        fs: { '/firmware.patch': { sha256: hash, size: payload.length } },
      }),
    ),
  );
  assets.set(
    'patches.json',
    encode(
      JSON.stringify({
        schema: 2,
        release: tag,
        patches: { [hash]: 'x70-110.fmpatch' },
        firmwares: [
          {
            id: 'x70-110',
            model: 'X70',
            version: '1.10',
            sha256: hash,
            bytes: 1,
            release_date: '2026-11-13',
            download_url: 'https://example.org/FPUPDATE.DAT',
            patch_sha256: hash,
            patch_bytes: payload.length,
            boot_instructions: ['Copy files.'],
            mods: ['fujimod'],
          },
        ],
        sd_card_files: card,
      }),
    ),
  );
  return { assets, hash, payload };
}

async function selectedPlan(release: LoadedRelease, fetcher: typeof fetch) {
  const cameras = release.firmwares.map((entry) => ({
    id: entry.id.toUpperCase(),
    model: entry.model,
    version: entry.version,
    processor: 'EXR Processor II',
    mods: entry.mods.length,
    download_url: entry.download_url,
  }));
  const manifests = await Promise.all(
    cameras.map(
      async (camera) =>
        [camera.id, await loadOtaRelease(release.assetBase, camera.id, fetcher)] as const,
    ),
  );
  const plan = sdBootV2Plan({
    cameras,
    releases: new Map(manifests),
    card: release.sd_card_files ?? [],
    excluded: new Set(),
  });
  expect(plan.problems).toEqual([]);
  return plan;
}

test.each([false, true])(
  'selected release survives rollover with cold caches (tampered=%s)',
  async (tampered) => {
    const a = await fixture('A'),
      b = await fixture('B');
    const server = releaseServer(async (tag, name) => {
      const bytes = (tag === 'A' ? a : tag === 'B' ? b : undefined)?.assets.get(name);
      return tampered && tag === 'A' && name === storeName(a.hash) ? b.payload : bytes;
    });
    const selected = await loadRelease(origin, server.fetch);
    expect(selected?.tag).toBe('A');
    if (!selected) throw Error('Release A unavailable');
    const plan = await selectedPlan(selected, server.fetch);
    expect(server.upstreamPaths).not.toContain(
      `/justjake/fujimod/releases/download/A/${storeName(a.hash)}`,
    );
    server.current = 'B';
    server.cold();
    expect((await loadRelease(origin, server.fetch))?.tag).toBe('B');
    const current = await server.fetch(`${origin}/ota/X70-110/latest.json`);
    expect((await current.json()).version).toBe('B');
    // The former download URL cannot retrieve A from B on this cold edge.
    expect((await server.fetch(`${origin}/ota/store/${storeName(a.hash)}`)).status).toBe(404);
    const start = server.publicPaths.length;
    const download = buildCard(plan.files, {
      assetBase: selected.assetBase,
      release: selected.release,
      git: false,
      fetcher: server.fetch,
    });
    if (tampered) {
      await expect(download).rejects.toThrow(/integrity/);
    } else {
      const files = unzipSync(new Uint8Array(await (await download).arrayBuffer()));
      expect(files[`FUJIMOD/SYSTEM/STORE/${storeName(a.hash)}`]).toEqual(a.payload);
      expect(JSON.parse(new TextDecoder().decode(files['FUJIMOD/SYSTEM/X70-110/A.JSN']))).toEqual({
        gen: 1,
        version: 'A',
        fs: { '/firmware.patch': a.hash },
      });
      expect(
        JSON.parse(new TextDecoder().decode(files['FUJIMOD/SYSTEM/X70-110/STATE.JSN'])),
      ).toEqual({
        a: { gen: 1, attempts: 0, confirmed: false },
        b: { gen: -1, attempts: 0, confirmed: false },
        stage2: { attempts: 0, confirmed: false },
      });
      expect(files['FFBOOT.BIN']).toEqual(a.assets.get('FFBOOT.BIN'));
    }
    expect(server.publicPaths.slice(start)).toEqual(
      plan.files
        .filter((file) => file.from !== 'data')
        .map(
          (file) =>
            `/releases/A/${file.from === 'release' ? file.asset : file.from === 'store' ? file.name : ''}`,
        ),
    );
  },
);

const artifacts = process.env.FUJIMOD_PATCHES;
test.skipIf(!artifacts)(
  'published asset directory passes Worker discovery and SD-card download',
  async () => {
    if (!artifacts) throw Error('FUJIMOD_PATCHES is required');
    const index = JSON.parse(await readFile(join(artifacts, 'patches.json'), 'utf8')) as {
      release: string;
    };
    const server = releaseServer(async (tag, name) => {
      if (tag !== index.release) return undefined;
      try {
        return new Uint8Array(await readFile(join(artifacts, name)));
      } catch (error) {
        if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return undefined;
        throw error;
      }
    });
    server.current = index.release;
    const selected = await loadRelease(origin, server.fetch);
    if (!selected) throw Error('Release unavailable');
    const plan = await selectedPlan(selected, server.fetch);
    server.cold();
    const zip = await buildCard(plan.files, {
      assetBase: selected.assetBase,
      release: selected.release,
      git: false,
      fetcher: server.fetch,
    });
    const files = unzipSync(new Uint8Array(await zip.arrayBuffer()));
    expect(Object.keys(files).sort()).toEqual(plan.files.map((file) => file.path).sort());
    for (const file of plan.files) {
      expect(files[file.path].length).toBe(file.from === 'data' ? file.data.length : file.bytes);
      if (file.from === 'data') expect(files[file.path]).toEqual(file.data);
      else expect(await sha256(files[file.path])).toBe(file.sha256);
    }
  },
);
