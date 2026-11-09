/** Check both real ZIP downloads from supplied official updates, without hardware. */
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { unzipSync } from 'fflate';
import { chromium, webkit } from 'playwright';
import { createHash } from 'node:crypto';
import { catalog, initialize, sdCardTools } from 'fujimod';

const cases: { firmwareId: string; dat: string }[] = JSON.parse(
  process.env.FUJIMOD_RECOVERY_CASES ?? '[]',
);
const expectedFiles: Record<string, Record<string, { bytes: number; sha256: string }>> = JSON.parse(
  process.env.FUJIMOD_RECOVERY_EXPECTED ?? '{}',
);
const expectedBuildSha = process.env.FUJIMOD_TEST_BUILD_SHA;
if (expectedBuildSha) assert.match(expectedBuildSha, /^[a-f0-9]{8}$/);
assert.ok(cases.length, 'Set FUJIMOD_RECOVERY_CASES to official update test cases');
await initialize({
  module_or_path: await readFile(
    new URL('../../../../bindings/javascript/pkg/fujimod_bg.wasm', import.meta.url),
  ),
});
const base = new URL('recovery/', process.env.FUJIMOD_WEB_URL ?? 'http://127.0.0.1:5190/').href;
const output = new URL('../../build/recovery-downloads/', import.meta.url);
await mkdir(output, { recursive: true });
const engines = [chromium, webkit].filter(
  (engine) =>
    !process.env.FUJIMOD_TEST_BROWSER || process.env.FUJIMOD_TEST_BROWSER === engine.name(),
);
assert.ok(engines.length, 'Choose chromium or webkit for FUJIMOD_TEST_BROWSER');
for (const engine of engines) {
  console.log(`${engine.name()}: opening recovery page`);
  const browser = await engine.launch();
  try {
    for (const test of cases) {
      const context = await browser.newContext({
        viewport: { width: 390, height: 844 },
        acceptDownloads: true,
      });
      const page = await context.newPage();
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.setDefaultTimeout(240000);
      await page.goto(base);
      await page.getByRole('searchbox', { name: 'Search cameras' }).waitFor({ state: 'visible' });
      console.log(`${engine.name()}: page loaded`);
      console.log(`${engine.name()}: selecting ${test.firmwareId}`);
      await page.locator('#recovery-file').setInputFiles(test.dat);
      const release = catalog
        .firmwareVersions()
        .find((entry) => entry.firmware_id === test.firmwareId);
      assert.ok(release);
      const identity = `${release.product.toLowerCase().replace(/\s+/g, '-')}-${release.version}`;
      await page
        .getByRole('radio', {
          name: `${release.product} · ${release.version}`,
          exact: true,
        })
        .check();
      const led = page.getByRole('button', { name: 'Download green LED image', exact: true });
      const system = page.getByRole('button', { name: 'Download full system image', exact: true });
      await led.waitFor({ state: 'visible' });
      const status = sdCardTools.availability(test.firmwareId);
      try {
        const pending = Promise.race([
          page.waitForEvent('download'),
          page
            .locator('.recovery-error[role=alert]')
            .waitFor({ state: 'visible' })
            .then(async () => {
              throw Error(await page.locator('.recovery-error[role=alert]').innerText());
            }),
        ]);
        await led.click();
        console.log(`${engine.name()}: building ${test.firmwareId} LED ZIP`);
        const download = await pending;
        const buildSha = download.suggestedFilename().match(/-build-([a-f0-9]{8})\.zip$/)?.[1];
        assert.ok(buildSha, 'Download names identify the serving application build');
        if (expectedBuildSha) assert.equal(buildSha, expectedBuildSha);
        assert.equal(
          download.suggestedFilename(),
          `fujimod-green-led-${identity}-build-${buildSha}.zip`,
        );
        const zip = unzipSync(await readFile((await download.path())!));
        assert.deepEqual(Object.keys(zip).sort(), [...status.greenLedFilenames].sort());
        assert.ok(
          zip['ffboot.bin']
            ? zip['ffboot.bin'].length < 1024
            : Object.values(zip).every((bytes) => bytes.length < 1024),
        );
        assert.ok(Object.values(zip).reduce((size, bytes) => size + bytes.length, 0) < 65536);
        const expected = sdCardTools.buildGreenLed(test.firmwareId, await readFile(test.dat));
        try {
          for (const file of expected.takeFiles()) {
            try {
              assert.deepEqual(zip[file.name], file.bytes);
            } finally {
              file.free();
            }
          }
        } finally {
          expected.free();
        }
        assert.equal(
          await page.getByRole('heading', { name: 'Flash the firmware as usual' }).count(),
          0,
        );
        assert.ok((await page.locator('main').innerText()).includes('green LED lights'));
        if (status.canBuild) {
          const pendingSystem = Promise.race([
            page.waitForEvent('download'),
            page
              .locator('.recovery-error[role=alert]')
              .waitFor({ state: 'visible' })
              .then(async () => {
                throw Error(await page.locator('.recovery-error[role=alert]').innerText());
              }),
          ]);
          await system.click();
          console.log(`${engine.name()}: building full system ZIP`);
          const systemDownload = await pendingSystem;
          assert.equal(
            systemDownload.suggestedFilename(),
            `fujimod-sd-card-recovery-${identity}-build-${buildSha}.zip`,
          );
          const files = unzipSync(await readFile((await systemDownload.path())!));
          assert.deepEqual(Object.keys(files).sort(), [...status.outputFilenames].sort());
          const checked = expectedFiles[test.firmwareId];
          if (checked) {
            assert.deepEqual(Object.keys(files).sort(), Object.keys(checked).sort());
            for (const [name, bytes] of Object.entries(files)) {
              assert.equal(bytes.length, checked[name].bytes);
              assert.equal(createHash('sha256').update(bytes).digest('hex'), checked[name].sha256);
            }
          }
          const expectedSystem = sdCardTools.build(test.firmwareId, await readFile(test.dat));
          try {
            for (const file of expectedSystem.takeFiles()) {
              try {
                assert.deepEqual(files[file.name], file.bytes);
              } finally {
                file.free();
              }
            }
          } finally {
            expectedSystem.free();
          }
          assert.equal(
            await page.getByRole('heading', { name: 'Flash the firmware as usual' }).count(),
            1,
          );
        } else {
          await system.waitFor({ state: 'visible' });
          assert.equal(await system.isDisabled(), true);
        }
        assert.deepEqual(errors, []);
        console.log(
          `${engine.name()}: ${test.firmwareId} green LED ZIP${status.canBuild ? ' and full system ZIP' : ''}`,
        );
      } finally {
        status.free();
      }
      await page.screenshot({
        path: new URL(`${engine.name()}.png`, output).pathname,
        fullPage: true,
      });
      assert.equal(
        await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
        false,
      );
      await context.close();
    }
  } finally {
    await browser.close();
  }
}
