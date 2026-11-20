import { expect, test } from 'vite-plus/test';
import {
  parseReportLink,
  validateReport,
  decode64,
  base64url,
  listedReport,
  reportDetails,
  reportEntries,
} from './report-link';
import { tar, link, hash, reportTar } from './fixtures.test-helper';
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
  ])
    expect(() => parseReportLink(hash(value as typeof link))).toThrow();
  const damaged = tar.slice();
  damaged[512] ^= 1;
  expect(validateReport(damaged, link).size).toBe(tar.length);
  const invalid = tar.slice();
  invalid[0] ^= 1;
  expect(() => validateReport(invalid, link)).toThrow('checksum');
});

test('the camera QR carries only compact retrieval metadata', () => {
  const encoded = hash().split('=')[1];
  const wire = JSON.parse(new TextDecoder().decode(decode64(encoded)));
  expect(wire).toEqual({
    v: 1,
    b: link.commit,
    r: '1234567',
    n: tar.length,
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
  const checksumLink = {
    v: 1,
    b: link.commit,
    r: '1234567',
    n: tar.length,
    c: '00000000',
    f: link.files,
  };
  expect(() =>
    parseReportLink(
      '#?report=' + base64url(new TextEncoder().encode(JSON.stringify(checksumLink))),
    ),
  ).toThrow();
});
test('a listed report uses manifest sizes and its file name', () => {
  const video = Uint8Array.of(100, 200, 128, 128);
  const bytes = reportTar(
    { 'screen/video.yuv': video, 'state.json': Uint8Array.of(123, 125) },
    '2026-10-27T14:04:05',
    { screen: { video: 'screen/video.yuv', width: 2, height: 1 } },
  );
  const { link: listed } = listedReport(bytes, link, 'R00000AB.TAR');
  expect(listed.path).toBe('A:\\FUJIMOD\\REPORTS\\R00000AB.TAR');
  expect(listed.bytes).toBe(bytes.length);
  const entries = reportEntries(bytes, listed);
  const details = reportDetails(bytes, entries);
  expect(details.capturedAt).toBe('2026-10-27T14:04:05');
  expect([...details.screen!.subarray(54, 60)]).toEqual([100, 100, 100, 200, 200, 200]);
  const damaged = bytes.slice();
  damaged[entries.find((entry) => entry.name === 'screen/video.yuv')!.offset + 2] ^= 1;
  expect(listedReport(damaged, link, 'R00000AB.TAR').link.bytes).toBe(damaged.length);
  expect(() => listedReport(bytes, link, 'NOTES.TXT')).toThrow();
});
