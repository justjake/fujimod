import { expect, test } from 'vite-plus/test';
import { crc32 } from '@fujimod/emulator-ui/archive';
import { parseReportLink, validateReport, decode64, base64url } from './report-link';
import { tar, link, hash } from './fixtures.test-helper';
test('decodes the fragment locally and preserves the entire inline TAR', async () => {
  const value = { ...link, archive: base64url(tar) };
  expect(parseReportLink(hash(value))).toEqual(value);
  const blob = validateReport(decode64(value.archive), value);
  expect(new Uint8Array(await blob.arrayBuffer())).toEqual(tar);
  expect(blob.type).toBe('application/x-tar');
});
test('rejects malformed descriptors and damaged archives', () => {
  for (const value of [
    { ...link, v: 2 },
    { ...link, bytes: -1 },
    { ...link, path: 'A:\\SECRET.TAR' },
    { ...link, crc32: 'wrong' },
  ])
    expect(() => parseReportLink(hash(value as typeof link))).toThrow();
  const damaged = tar.slice();
  damaged[512] ^= 1;
  expect(() => validateReport(damaged, link)).toThrow('incomplete');
  const invalid = tar.slice();
  invalid[0] ^= 1;
  expect(() =>
    validateReport(invalid, { ...link, crc32: crc32(invalid).toString(16).padStart(8, '0') }),
  ).toThrow('checksum');
});

test('the camera QR carries only compact retrieval metadata', () => {
  const encoded = hash().split('=')[1];
  const wire = JSON.parse(new TextDecoder().decode(decode64(encoded)));
  expect(wire).toEqual({
    v: 1,
    b: link.commit,
    r: '1234567',
    n: tar.length,
    c: link.crc32,
    f: link.files,
  });
  expect(('https://fujimod.com/bug' + hash()).length).toBeLessThan(190);
  expect(parseReportLink(hash())).toEqual(link);
  for (const changes of [
    { b: 'a'.repeat(40) },
    { b: 12345678 },
    { r: '../x' },
    { firmware: 'x70-110' },
  ])
    expect(() =>
      parseReportLink(
        '#?report=' + base64url(new TextEncoder().encode(JSON.stringify({ ...wire, ...changes }))),
      ),
    ).toThrow();
});
