import { expect, test } from 'vite-plus/test';
import type { SdCardFile, PatchFirmware } from '../../patch/catalog';
import type { Firmware } from '../../catalog';
import { storeName, validateOtaRelease, type SdBootV2Release } from '../../ota/release';
import { sdBootV2Files, sdBootV2Plan, conflicting, releaseCameras, type CardCamera } from './plan';

const digest = (c: string) => c.repeat(64);
const release = (id: string, mods: number, sha: string) =>
  ({
    id,
    model: id.toUpperCase(),
    version: '1.00',
    sha256: sha,
    mods: Array(mods).fill('m'),
  }) as PatchFirmware;
const camera = (id: string, processor = 'EXR Processor II'): CardCamera => ({
  id,
  model: id,
  version: '1.00',
  processor,
  mods: 1,
});
const file = (path: string, sha: string, processor?: string): SdCardFile => ({
  path,
  asset: storeName(sha),
  sha256: sha,
  bytes: 3,
  ...(processor ? { processor } : {}),
});

test('the checklist lists cameras with a known processor, most mods first', () => {
  const catalog = [
    { sha256: digest('a'), processor: 'EXR Processor II' },
    { sha256: digest('b'), processor: 'EXR Processor II' },
  ] as Firmware[];
  const cameras = releaseCameras(
    [
      release('x70-110', 3, digest('a')),
      release('xe2-411', 9, digest('b')),
      release('x-c', 20, digest('c')),
    ],
    catalog,
  );
  expect(cameras.map((entry) => [entry.id, entry.mods])).toEqual([
    ['XE2-411', 9],
    ['X70-110', 3],
  ]);
});

test('store names and release manifests follow the camera update layer', () => {
  expect(storeName('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855')).toBe(
    'E3B0C442.98F',
  );
  const fs = { '/firmware.patch': { sha256: digest('a'), size: 3 } };
  expect(validateOtaRelease({ firmware: 'X70-110', version: '1', fs }, 'X70-110').fs).toEqual(fs);
  for (const value of [
    { firmware: 'XE2-411', version: '1', fs },
    { firmware: 'X70-110', version: '1', fs: { 'firmware.patch': fs['/firmware.patch'] } },
    { firmware: 'X70-110', version: '1', fs: { '/a/../b': fs['/firmware.patch'] } },
    // Two contents under one store name would overwrite each other on the card.
    {
      firmware: 'X70-110',
      version: '1',
      fs: {
        '/a': { sha256: digest('a'), size: 1 },
        '/b': { sha256: `${'a'.repeat(11)}${'b'.repeat(53)}`, size: 1 },
      },
    },
  ])
    expect(() => validateOtaRelease(value, 'X70-110')).toThrow();
});

test('the plan writes boot files, chosen card files, both manifests and one copy of each store file', () => {
  const card = [
    file('FFBOOT.BIN', digest('1'), 'EXR Processor II'),
    file('FFSYS.BIN', digest('2'), 'EXR Processor II'),
    file('FM_EXR2.BIN', digest('3'), 'EXR Processor II'),
    file('FUJIMOD/RECIPES/A.JSN', digest('4')),
    file('FUJIMOD/RECIPES/B.JSN', digest('5')),
  ];
  const shared = { sha256: digest('6'), size: 10 };
  const releases = new Map<string, SdBootV2Release>([
    ['X70-110', { firmware: 'X70-110', version: '1', fs: { '/etc/ssl/cert.pem': shared } }],
    [
      'XE2-411',
      {
        firmware: 'XE2-411',
        version: '1',
        fs: { '/etc/ssl/cert.pem': shared, '/firmware.patch': { sha256: digest('7'), size: 5 } },
      },
    ],
  ]);
  const plan = sdBootV2Plan({
    cameras: [camera('X70-110'), camera('XE2-411')],
    card,
    releases,
    excluded: new Set(['FUJIMOD/RECIPES/B.JSN']),
  });
  expect(plan.problems).toEqual([]);
  expect(plan.files.map((entry) => entry.path).sort()).toEqual([
    'FFBOOT.BIN',
    'FFSYS.BIN',
    'FM_EXR2.BIN',
    'FUJIMOD/RECIPES/A.JSN',
    `FUJIMOD/SYSTEM/STORE/${storeName(digest('6'))}`,
    `FUJIMOD/SYSTEM/STORE/${storeName(digest('7'))}`,
    'FUJIMOD/SYSTEM/X70-110/A.JSN',
    'FUJIMOD/SYSTEM/X70-110/STATE.JSN',
    'FUJIMOD/SYSTEM/XE2-411/A.JSN',
    'FUJIMOD/SYSTEM/XE2-411/STATE.JSN',
  ]);
  const manifest = plan.files.find((entry) => entry.path === 'FUJIMOD/SYSTEM/XE2-411/A.JSN')!;
  expect(manifest.from === 'data' && JSON.parse(new TextDecoder().decode(manifest.data))).toEqual({
    gen: 1,
    version: '1',
    fs: { '/etc/ssl/cert.pem': digest('6'), '/firmware.patch': digest('7') },
  });
});

test('missing boot files or release manifests stop the download', () => {
  const plan = sdBootV2Plan({
    cameras: [camera('X70-110')],
    card: [file('FFBOOT.BIN', digest('1'), 'EXR Processor II')],
    releases: new Map(),
    excluded: new Set(),
  });
  expect(plan.problems).toEqual([
    'This release has no FFSYS.BIN, FM_EXR2.BIN for EXR Processor II cameras.',
    'The X70-110 release files are unavailable.',
  ]);
  expect(
    sdBootV2Plan({ cameras: [], card: [], releases: new Map(), excluded: new Set() }).problems,
  ).toHaveLength(1);
  expect(sdBootV2Files([], 'Other').missing).toEqual([]);
});

test('cameras whose root files differ from the chosen cameras’ cannot join the card', () => {
  const card = [
    file('FFBOOT.BIN', digest('1'), 'EXR Processor II'),
    file('FFBOOT.BIN', digest('2'), 'X-Processor Pro'),
    file('FM_XPP.BIN', digest('3'), 'X-Processor Pro'),
  ];
  const all = [camera('X70-110'), camera('XE2-411'), camera('XT20-211', 'X-Processor Pro')];
  expect([...conflicting(all, [all[0]], card)]).toEqual(['XT20-211']);
  expect([...conflicting(all, [], card)]).toEqual([]);
});
