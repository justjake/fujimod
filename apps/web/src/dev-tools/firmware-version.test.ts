import { readFile } from 'node:fs/promises';
import { beforeAll, expect, test } from 'vite-plus/test';
import { catalog, initialize } from 'fujimod';
import { firmwareVersion } from './firmware-version';

beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(
      new URL('../../../../bindings/javascript/pkg/fujimod_bg.wasm', import.meta.url),
    ),
  });
});

const status = (product: string, internalVersion: string) =>
  firmwareVersion({ product, internalVersion }, catalog.firmwareVersions());

test('both contributed identities map to public releases independently of bulk support', () => {
  expect(status('X-T1', '2.39')).toMatchObject({ publicVersion: '5.52', outdated: false });
  expect(status('X-H1', '1.90')).toMatchObject({ publicVersion: '2.15', outdated: false });
});

test('known older and unknown lower internal versions are flagged within their own model', () => {
  expect(status('X-M1', '1.61')).toMatchObject({ publicVersion: '1.01', outdated: true });
  expect(status('FUJIFILM X-T1', ' 2.09 ')).toMatchObject({
    publicVersion: undefined,
    outdated: true,
  });
  expect(status('X-M1', '1.79')).toMatchObject({ publicVersion: undefined, outdated: false });
  expect(status('X-S10', '1.90')).toMatchObject({ publicVersion: '3.11', outdated: false });
  expect(status('unrecognized', '1.00')).toBeUndefined();
  expect(status('X-T1', 'SYNTHETIC')).toBeUndefined();
});
