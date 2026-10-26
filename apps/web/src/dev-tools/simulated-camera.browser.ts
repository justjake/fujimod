/** Real stock execution check. The server supplies a qualified USB Wasm graph and DAT. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import type { Catalog, Firmware } from '../catalog';
import type { CameraStore } from '../cameras/store';
import type { CameraModel } from './camera-model';
declare global {
  interface Window {
    __fujimodCameraModel?: CameraModel;
  }
}

const url = new URL('dev-tools/', process.env.FUJIMOD_WEB_URL || 'http://127.0.0.1:5204/');
const browser = await chromium.launch({ headless: true });
const directory = new URL('../../build/simulated-camera/', import.meta.url);
await mkdir(directory, { recursive: true });
async function verify(firmware: Firmware): Promise<string> {
  assert.ok(firmware.emulate?.usb, `${firmware.id}: missing USB runtime`);
  const output = new URL(firmware.id + '/', directory);
  await mkdir(output, { recursive: true });
  const context = await browser.newContext({ viewport: { width: 1300, height: 900 } });
  const page = await context.newPage();
  const errors: string[] = [];
  console.log(`CHECK ${firmware.id}`);
  try {
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text());
    });
    await page.addInitScript(() => {
      const NativeWorker = Worker;
      globalThis.Worker = class extends NativeWorker {
        constructor(...args: ConstructorParameters<typeof Worker>) {
          super(...args);
          this.addEventListener('message', (event: MessageEvent<{ type?: string }>) => {
            if (event.data?.type === 'error')
              console.error('Simulation worker fault: ' + JSON.stringify(event.data));
          });
        }
      };
    });
    await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: 90_000 });
    const savedId = await page.evaluate(async (firmwareId) => {
      const moduleUrl = new URL('/src/cameras/store.ts', location.href).href;
      const module = (await import(/* @vite-ignore */ moduleUrl)) as {
        CameraStore: typeof CameraStore;
      };
      const catalog = (await (await fetch('/runtime/catalog.json')).json()) as Catalog;
      const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId)!;
      if (!firmware) throw Error('No qualified USB runtime in the catalog.');
      const response = await fetch(`/local/firmware/${firmwareId}.dat`);
      if (!response.ok) throw Error(`Firmware mount failed: ${firmwareId} (${response.status})`);
      const bytes = await response.arrayBuffer();
      return (
        await new module.CameraStore().create(new File([bytes], 'camera.dat'), firmware, false)
      ).id;
    }, firmware.id);
    await page.reload({ waitUntil: 'domcontentloaded', timeout: 90_000 });
    await page.waitForFunction(() => 'connectSimulated' in (window.__fujimodCameraModel ?? {}));
    const tile = page.locator('.td-simulated');
    await tile.getByRole('button').click();
    const state = await page.waitForFunction(
      () => {
        const model = (window as Window & { __fujimodCameraModel?: CameraModel })
          .__fujimodCameraModel;
        const camera = model?.cameras().find((camera) => camera.transport === 'simulated');
        const error = document.querySelector('[role=alert]')?.textContent;
        if (error) throw Error(error);
        if (camera?.problem) throw Error(camera.problem);
        return camera?.identity?.product && !camera.session ? camera : undefined;
      },
      undefined,
      { timeout: 180_000 },
    );
    const camera = await state.jsonValue();
    assert.ok(camera?.identity);
    assert.equal(camera.id, `simulated-${savedId}`);
    assert.equal(camera.present, true);
    assert.equal(new URL(page.url()).pathname, url.pathname);
    const proof = await page.evaluate(async () => {
      const model = window.__fujimodCameraModel!;
      const camera = model.cameras().find((camera) => camera.transport === 'simulated')!;
      const identity = await model.withSession(
        camera.id,
        'Browser identity verification',
        (session) => session.deviceInfo('Product'),
      );
      const entries = model
        .log()
        .filter(
          (entry) =>
            entry.camera === camera.id && entry.message.startsWith('Device info · Product →'),
        );
      if (!entries.length) throw Error('No Rust-owner decoded identity exchange.');
      return { identity: identity.text, entries, camera: model.cameras()[0] };
    });
    assert.ok(proof.identity, `${firmware.id}: empty guest identity`);
    assert.equal(proof.identity, camera.identity.product);
    const model = (text: string) =>
      text
        .trim()
        .toUpperCase()
        .replace(/^FUJIFILM\s+/, '');
    assert.equal(model(proof.identity), model(firmware.model), `${firmware.id}: wrong guest model`);
    assert.ok(
      proof.entries.every(
        (entry) => entry.sizes && entry.sizes.sent > 0 && entry.sizes.received > 0,
      ),
    );
    await page.getByText(proof.identity, { exact: true }).first().waitFor({ state: 'visible' });
    await page.screenshot({ path: new URL('connected.png', output).pathname, fullPage: true });
    await page.evaluate(async () => {
      const model = window.__fujimodCameraModel!;
      await model.remove(model.cameras().find((camera) => camera.transport === 'simulated')!.id);
      if (model.cameras().some((camera) => camera.transport === 'simulated'))
        throw Error('Simulation was not removed.');
    });
    assert.deepEqual(errors, []);
    await writeFile(new URL('identity.json', output), JSON.stringify(proof, null, 2));
    console.log(
      `PASS ${firmware.id} saved camera ${savedId}: ${proof.identity}, Rust owner log, no navigation, removal`,
    );
    return proof.identity;
  } catch (error) {
    await page.screenshot({ path: new URL('failure.png', output).pathname, fullPage: true });
    await writeFile(
      new URL('failure.json', output),
      JSON.stringify(
        {
          firmware: firmware.id,
          error: error instanceof Error ? error.stack : String(error),
          errors,
        },
        null,
        2,
      ),
    );
    throw error;
  } finally {
    await context.close();
  }
}
const results: { firmware: string; passed: boolean; identity?: string; error?: string }[] = [];
try {
  const context = await browser.newContext();
  const response = await context.request.get(new URL('/runtime/catalog.json', url).href);
  assert.ok(response.ok(), 'Runtime catalog is unavailable');
  const catalog = (await response.json()) as Catalog;
  await context.close();
  const selected = process.env.FUJIMOD_TEST_FIRMWARE;
  const firmwares = catalog.firmwares.filter(
    (entry) => entry.emulate && (!selected || entry.id === selected),
  );
  assert.ok(firmwares.length, 'No catalog firmware selected');
  for (const firmware of firmwares) {
    try {
      const identity = await verify(firmware);
      results.push({ firmware: firmware.id, passed: true, identity });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.error(`FAIL ${firmware.id}: ${message}`);
      results.push({ firmware: firmware.id, passed: false, error: message });
    }
  }
  await writeFile(new URL('results.json', directory), JSON.stringify(results, null, 2));
  assert.deepEqual(
    results.filter((entry) => !entry.passed),
    [],
    'Every selected firmware must connect',
  );
} finally {
  await browser.close();
}
