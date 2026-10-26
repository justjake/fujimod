/** Real emulator integration with Chromium's virtual camera, never host camera hardware. */
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';

declare global {
  interface Window {
    webcamFrames: { width: number; height: number }[];
    webcamStream?: MediaStream;
  }
}

const url = process.env.FUJIMOD_WEB_URL;
const dat = process.env.FUJIMOD_TEST_DAT;
if (!url || !dat) throw Error('Set FUJIMOD_WEB_URL and FUJIMOD_TEST_DAT.');
const output = new URL('../../build/webcam/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: true,
  args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'],
});
try {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await context.routeWebSocket('**/*', () => {});
  await context.addInitScript(() => {
    window.webcamFrames = [];
    const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async (constraints) => {
      const stream = await getUserMedia(constraints);
      window.webcamStream = stream;
      return stream;
    };
    const OriginalWorker = window.Worker;
    window.Worker = class extends OriginalWorker {
      override postMessage(
        message: unknown,
        options?: Transferable[] | StructuredSerializeOptions,
      ) {
        const data = message as {
          type?: string;
          request?: { action?: string; scene?: { width: number; height: number } };
        };
        if (data.type === 'rpc' && ['boot', 'scene'].includes(data.request?.action ?? '')) {
          const scene = data.request?.scene;
          if (scene) window.webcamFrames.push({ width: scene.width, height: scene.height });
        }
        if (Array.isArray(options)) super.postMessage(message, options);
        else super.postMessage(message, options);
      }
    };
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (/\[[A-Z_]+\]/.test(message.text())) errors.push(message.text());
  });
  await page.goto(new URL('#workspace', url).href);
  await page.locator('#viewer-tools[data-ready="true"]').waitFor({ state: 'attached' });
  console.log('Viewer ready; selecting firmware.');
  const chooser = page.waitForEvent('filechooser');
  await page.locator('#firmware-prompt').click();
  await (await chooser).setFiles(dat);
  console.log('Firmware selected; waiting for the real camera boot.');
  await page.waitForFunction(
    () => document.querySelector('#live-label')?.textContent === 'LIVE',
    undefined,
    { timeout: 240000 },
  );
  const initial = await page.evaluate(() => window.webcamFrames.at(-1));
  assert.ok(initial, 'The default scene must reach the emulator before enabling webcam input.');
  console.log('Camera ready; enabling the virtual webcam.');
  const before = await page.evaluate(() => window.webcamFrames.length);
  await page.getByRole('button', { name: 'Enable webcam input', exact: true }).click();
  await page.waitForFunction(
    (count) => window.webcamFrames.length > count && window.webcamFrames.at(-1)!.width <= 640,
    before,
  );
  assert.equal(await page.locator('#webcam-toggle').getAttribute('aria-pressed'), 'true');
  assert.equal(await page.evaluate(() => window.webcamStream?.getAudioTracks().length), 0);
  await page.waitForFunction(
    () => Number(document.querySelector('#viewer')?.getAttribute('data-frames')) > 2,
  );
  // Chromium's virtual webcam supplies a green test scene. Observe the guest
  // LCD, not just the host's submitted sensor message or the toggle state.
  await page.waitForFunction(
    () => {
      const canvas = document.querySelector<HTMLCanvasElement>('#lcd-source')!;
      const { data } = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height);
      let green = 0,
        samples = 0;
      for (let y = 80; y < canvas.height - 80; y += 8)
        for (let x = 80; x < canvas.width - 80; x += 8) {
          const i = (y * canvas.width + x) * 4;
          samples++;
          if (data[i + 1] > data[i] * 1.2 && data[i + 1] > data[i + 2] * 1.2) green++;
        }
      return green > samples / 4;
    },
    undefined,
    { timeout: 60000 },
  );
  await page.screenshot({ path: new URL('enabled.png', output).pathname });
  await page.getByRole('button', { name: 'Disable webcam input', exact: true }).click();
  await page.waitForFunction((size) => {
    const last = window.webcamFrames.at(-1);
    return last?.width === size.width && last.height === size.height;
  }, initial);
  assert.equal(
    await page.evaluate(() =>
      window.webcamStream?.getTracks().every((track) => track.readyState === 'ended'),
    ),
    true,
  );
  assert.equal(await page.locator('#webcam-toggle').getAttribute('aria-pressed'), 'false');
  await page.waitForFunction(
    () => {
      const canvas = document.querySelector<HTMLCanvasElement>('#lcd-source')!;
      const { data } = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height);
      let green = 0,
        samples = 0;
      for (let y = 80; y < canvas.height - 80; y += 8)
        for (let x = 80; x < canvas.width - 80; x += 8) {
          const i = (y * canvas.width + x) * 4;
          samples++;
          if (data[i + 1] > data[i] * 1.2 && data[i + 1] > data[i + 2] * 1.2) green++;
        }
      return green < samples / 10;
    },
    undefined,
    { timeout: 60000 },
  );
  assert.deepEqual(errors, []);
  await page.screenshot({ path: new URL('disabled.png', output).pathname });
  await writeFile(
    new URL('result.json', output),
    JSON.stringify(
      { url, initial, frames: await page.evaluate(() => window.webcamFrames.length), errors },
      null,
      2,
    ),
  );
  console.log(
    'Webcam input reached the real emulator; disabling stopped all tracks and restored the default scene.',
  );
} finally {
  await browser.close();
}
