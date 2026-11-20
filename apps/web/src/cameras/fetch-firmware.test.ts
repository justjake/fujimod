import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { Firmware } from '../catalog';
import { sha256 } from '../patch/patcher';
import { fetchFirmware } from './fetch-firmware';
import { firmwareMirrors } from './firmware-mirrors';
import { sharingUrl } from '../sharing-server';
const bytes = new Uint8Array([0, 4, 255, 16]);
const firmware: Firmware = {
  id: 'x70-110',
  model: 'X70',
  version: '1.10',
  bytes: bytes.length,
  sha256: await sha256(bytes),
  download_url: 'https://dl.fujifilm-x.com/unused',
  release_date: '2016-06-09',
};
afterEach(() => vi.unstubAllGlobals());
test('fetches the French mirror through the proxy and returns verified bytes', async () => {
  const request = vi.fn(async () => new Response(bytes));
  vi.stubGlobal('fetch', request);
  const file = await fetchFirmware(firmware);
  expect(request).toHaveBeenCalledWith(
    sharingUrl(`/proxy?url=${encodeURIComponent(firmwareMirrors[firmware.id])}`),
  );
  expect(file.name).toBe('FPUPDATE.DAT');
  expect(new Uint8Array(await file.arrayBuffer())).toEqual(bytes);
});
test('rejects truncated or changed bytes, HTTP failures and missing mirrors', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(bytes.slice(1))),
  );
  await expect(fetchFirmware(firmware)).rejects.toThrow('integrity');
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(new Uint8Array(bytes.length))),
  );
  await expect(fetchFirmware(firmware)).rejects.toThrow('integrity');
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('', { status: 502 })),
  );
  await expect(fetchFirmware(firmware)).rejects.toThrow('502');
  const request = vi.fn();
  vi.stubGlobal('fetch', request);
  await expect(fetchFirmware({ ...firmware, id: 'missing' })).rejects.toThrow('No download mirror');
  expect(request).not.toHaveBeenCalled();
});
