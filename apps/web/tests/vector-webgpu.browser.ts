import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';

interface VectorDiagnostics {
  cpuRasterCalls: number;
  cpuVectorPaintCalls: number;
  gpuGlesLists: number;
  gpuVectorPaintCalls: number;
}
interface RuntimeManifest extends Record<string, unknown> {
  dmpCpuRenderer?: boolean;
}
const base = process.env.FUJIMOD_WEB_URL;
if (!base) throw Error('Set FUJIMOD_WEB_URL.');
const scenario = new URL('/?scenario=x70-lcd#workspace', base).href;
const output = new URL('../build/vector-webgpu/', import.meta.url);
await mkdir(output, { recursive: true });

const browser = await chromium.launch({
  headless: true,
  args: ['--enable-unsafe-webgpu', '--use-angle=metal'],
});

async function run(cpuRenderer: boolean, label: 'cpu' | 'webgpu') {
  const context = await browser.newContext({ viewport: { width: 1300, height: 1100 } });
  await context.routeWebSocket('**/*', () => {});
  await context.route('**/wasm.json', async (route) => {
    const response = await route.fetch();
    const manifest = (await response.json()) as RuntimeManifest;
    await route.fulfill({
      status: response.status(),
      headers: { ...response.headers(), 'content-type': 'application/json' },
      body: JSON.stringify({ ...manifest, dmpCpuRenderer: cpuRenderer }),
    });
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(scenario);
  await page.waitForFunction(
    () => window.cameraPerformance !== undefined,
    undefined,
    { timeout: 55000 },
  );
  const adapter = await page.evaluate(async () => {
    const gpuNavigator = navigator as Navigator & {
      gpu?: {
        requestAdapter(options?: { powerPreference?: string }): Promise<{
          info?: { vendor: string; architecture: string; device: string; description: string };
        } | null>;
      };
    };
    const adapter = await gpuNavigator.gpu?.requestAdapter({ powerPreference: 'high-performance' });
    const canvas = document.createElement('canvas');
    const gl = canvas.getContext('webgl2');
    const debug = gl?.getExtension('WEBGL_debug_renderer_info') as
      | { UNMASKED_RENDERER_WEBGL: number }
      | null;
    return {
      available: adapter !== undefined && adapter !== null,
      info: adapter?.info,
      renderer: debug && gl ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : '',
    };
  });
  assert.equal(adapter.available, true, JSON.stringify(adapter));
  if (adapter.info?.vendor) assert.equal(adapter.info.vendor, 'apple');
  if (adapter.info?.architecture) assert.equal(adapter.info.architecture, 'metal-3');
  assert.match(adapter.renderer, /Apple M2 Pro/);
  const canvas = page.locator('#lcd-source');
  const frame = () =>
    canvas.evaluate((source) => {
      if (!(source instanceof HTMLCanvasElement)) throw Error('LCD canvas is unavailable.');
      return source.toDataURL();
    });
  const rec = await frame();
  await page.screenshot({ path: new URL(`${label}-rec.png`, output).pathname, fullPage: true });
  await page.locator('button[data-id="menu_ok_button"]').click();
  await page.waitForFunction(
    (previous) =>
      document.querySelector<HTMLCanvasElement>('#lcd-source')?.toDataURL() !== previous,
    rec,
    { timeout: 55000 },
  );
  const menu = await frame();
  await page.screenshot({ path: new URL(`${label}-menu.png`, output).pathname, fullPage: true });
  const diagnostics = await page.evaluate(async () => {
    const engine = window.cameraEngine;
    if (!engine) throw Error('Camera engine is unavailable.');
    const states = (await engine.deviceStates('vector-gpu')).states;
    const counters: Record<string, number | undefined> = {};
    const visit = (value: unknown) => {
      if (Array.isArray(value)) {
        value.forEach(visit);
      } else if (value !== null && typeof value === 'object') {
        for (const key of [
          'cpu_raster_calls',
          'cpu_vector_paint_calls',
          'gpu_gles_lists',
          'gpu_vector_paint_calls',
        ]) {
          const candidate = Reflect.get(value, key);
          if (typeof candidate === 'number') counters[key] = candidate;
        }
        Object.values(value).forEach(visit);
      }
    };
    visit(states);
    for (const [key, value] of Object.entries(counters)) {
      if (value === undefined) throw Error(`Vector GPU diagnostic ${key} is unavailable.`);
    }
    return {
      cpuRasterCalls: counters.cpu_raster_calls!,
      cpuVectorPaintCalls: counters.cpu_vector_paint_calls!,
      gpuGlesLists: counters.gpu_gles_lists!,
      gpuVectorPaintCalls: counters.gpu_vector_paint_calls!,
    };
  });
  assert.deepEqual(errors, []);
  await context.close();
  return { rec, menu, adapter, diagnostics };
}

try {
  const cpu = await run(true, 'cpu');
  const webgpu = await run(false, 'webgpu');
  assert.equal(webgpu.rec, cpu.rec, 'REC screenshots differ between CPU and WebGPU.');
  assert.equal(webgpu.menu, cpu.menu, 'MENU screenshots differ between CPU and WebGPU.');
  assert.equal(webgpu.diagnostics.cpuRasterCalls, 0);
  assert.equal(webgpu.diagnostics.cpuVectorPaintCalls, 0);
  assert(webgpu.diagnostics.gpuGlesLists > 0, 'The vector GLES path did not use WebGPU.');
  assert(webgpu.diagnostics.gpuVectorPaintCalls > 0, 'Vector coverage/paint did not use WebGPU.');
  assert(cpu.diagnostics.cpuRasterCalls > 0, 'The CPU renderer baseline was not exercised.');
  assert(cpu.diagnostics.cpuVectorPaintCalls > 0, 'The CPU vector paint baseline was not exercised.');
  console.log(
    `${webgpu.adapter.renderer} (WebGPU ${webgpu.adapter.info?.vendor}/${webgpu.adapter.info?.architecture}): X70 REC and MENU match CPU; CPU raster/vector paint calls 0; vector GLES lists ${webgpu.diagnostics.gpuGlesLists}; vector GPU batches ${webgpu.diagnostics.gpuVectorPaintCalls}.`,
  );
} finally {
  await browser.close();
}
