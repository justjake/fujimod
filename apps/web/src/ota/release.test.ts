import { expect, test } from 'vite-plus/test';
import { loadOtaRelease, sdBootV2Firmware, sdBootV2SystemFolder } from './release';

const manifest = {
  firmware: 'X70-110',
  version: '0.1.0',
  fs: { '/firmware.patch': { sha256: 'a'.repeat(64), size: 12 } },
};

function fetcher(body: unknown, init: ResponseInit = { status: 200 }) {
  const urls: string[] = [];
  const fetch = async (input: RequestInfo | URL) => {
    urls.push(input instanceof URL ? input.href : input instanceof Request ? input.url : input);
    return new Response(JSON.stringify(body), {
      headers: { 'content-type': 'application/json' },
      ...init,
    });
  };
  return { urls, fetch: fetch as typeof globalThis.fetch };
}

test('the pinned release manifest is loaded from the release asset folder', async () => {
  const { urls, fetch } = fetcher(manifest);
  const release = await loadOtaRelease('https://fujimod.com/releases/0.1.0/', 'X70-110', fetch);
  expect(urls).toEqual(['https://fujimod.com/releases/0.1.0/X70-110-latest.json']);
  expect(release.firmware).toBe('X70-110');
  expect(release.fs['/firmware.patch']).toEqual({ sha256: 'a'.repeat(64), size: 12 });
});

test('a manifest that names another firmware is refused', async () => {
  const { fetch } = fetcher({ ...manifest, firmware: 'X-E2S-111' });
  await expect(loadOtaRelease('https://fujimod.com/releases/0.1.0/', 'X70-110', fetch)).rejects.toThrow(
    'invalid',
  );
});

test('a missing manifest is reported as unavailable', async () => {
  const { fetch } = fetcher({ message: 'not found' }, { status: 404 });
  await expect(loadOtaRelease('https://fujimod.com/releases/0.1.0/', 'X70-110', fetch)).rejects.toThrow(
    'unavailable',
  );
});

test('the release identity is the firmware id in upper case', () => {
  expect(sdBootV2Firmware('x70-110')).toBe('X70-110');
});

test('the card directory is an 8.3 name', () => {
  expect(sdBootV2SystemFolder('X70-110')).toBe('X70-110');
  expect(sdBootV2SystemFolder('XE2S-111')).toBe('XE2S-111');
  expect(sdBootV2SystemFolder('X100T-111')).toBe('X100T111');
  expect(() => sdBootV2SystemFolder('X100VI-131')).toThrow('no SD card folder name');
});
