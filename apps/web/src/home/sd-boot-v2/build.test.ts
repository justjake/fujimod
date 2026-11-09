import { expect, test } from 'vite-plus/test';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sha256 } from '../../patch/patcher';
import { storeName } from '../../ota/release';
import { buildCard } from './build';
import { sdBootV2Plan } from './plan';

test('the card ZIP holds every checked file and a clean Git repository of FUJIMOD', async () => {
  const encode = (text: string) => new TextEncoder().encode(text);
  const boot = encode('stage 2'),
    recipe = encode('{"name":"A"}\n'),
    patch = encode('patch bytes');
  const bootSha = await sha256(boot),
    recipeSha = await sha256(recipe),
    patchSha = await sha256(patch);
  const served = new Map<string, Uint8Array>([
    ['/releases/1.0/STAGE2.BIN', boot],
    ['/releases/1.0/RECIPE.JSN', recipe],
    [`/ota/store/${storeName(patchSha)}`, patch],
  ]);
  const fetcher = (async (input: URL | RequestInfo) => {
    const data = served.get(new URL(input instanceof Request ? input.url : input).pathname);
    return data ? new Response(data.slice()) : new Response(null, { status: 404 });
  }) as typeof fetch;
  const processor = 'EXR Processor II';
  const plan = sdBootV2Plan({
    cameras: [{ id: 'X70-110', model: 'X70', version: '1.10', processor, mods: 1 }],
    card: [
      ...['FFBOOT.BIN', 'FFSYS.BIN', 'FM_EXR2.BIN'].map((path) => ({
        path,
        asset: 'STAGE2.BIN',
        sha256: bootSha,
        bytes: boot.length,
        processor,
      })),
      {
        path: 'FUJIMOD/RECIPES/A.JSN',
        asset: 'RECIPE.JSN',
        sha256: recipeSha,
        bytes: recipe.length,
      },
    ],
    releases: new Map([
      [
        'X70-110',
        {
          firmware: 'X70-110',
          version: '1.0',
          fs: { '/firmware.patch': { sha256: patchSha, size: patch.length } },
        },
      ],
    ]),
    excluded: new Set(),
  });
  expect(plan.problems).toEqual([]);
  const zip = await buildCard(plan.files, {
    assetBase: 'https://fujimod.test/releases/1.0/',
    base: 'https://fujimod.test/',
    release: '1.0',
    git: true,
    fetcher,
  });
  const directory = await mkdtemp(join(tmpdir(), 'sd-card-zip-'));
  try {
    await writeFile(join(directory, 'card.zip'), new Uint8Array(await zip.arrayBuffer()));
    execFileSync('unzip', ['-q', 'card.zip', '-d', 'card'], { cwd: directory });
    const card = join(directory, 'card');
    expect(await readFile(join(card, 'FM_EXR2.BIN'), 'utf8')).toBe('stage 2');
    expect(await readFile(join(card, 'FUJIMOD/SYSTEM/STORE', storeName(patchSha)), 'utf8')).toBe(
      'patch bytes',
    );
    expect(JSON.parse(await readFile(join(card, 'FUJIMOD/SYSTEM/X70-110/A.JSN'), 'utf8'))).toEqual({
      gen: 1,
      version: '1.0',
      fs: { '/firmware.patch': patchSha },
    });
    const git = (...args: string[]) =>
      execFileSync('git', ['-c', 'safe.directory=*', ...args], {
        cwd: join(card, 'FUJIMOD'),
        encoding: 'utf8',
        env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
      });
    git('fsck', '--strict', '--no-dangling');
    expect(git('status', '--porcelain')).toBe('');
    expect(git('ls-files').trim().split('\n')).toEqual([
      'RECIPES/A.JSN',
      `SYSTEM/STORE/${storeName(patchSha)}`,
      'SYSTEM/X70-110/A.JSN',
      'SYSTEM/X70-110/STATE.JSN',
    ]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('a file whose bytes differ from the release is refused', async () => {
  const fetcher = (async () => new Response('tampered')) as typeof fetch;
  await expect(
    buildCard(
      [{ path: 'FFBOOT.BIN', from: 'release', asset: 'A.BIN', sha256: 'a'.repeat(64), bytes: 8 }],
      {
        assetBase: 'https://fujimod.test/releases/1/',
        base: 'https://fujimod.test/',
        release: '1',
        git: false,
        fetcher,
      },
    ),
  ).rejects.toThrow(/integrity/);
});
