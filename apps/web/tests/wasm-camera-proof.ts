import { createEngine } from '../src/engine';
import type { EngineConfig } from '@fujimod/emulator-ui/engine/api';
import { LCDViewer } from '@fujimod/emulator-ui/viewer/lcd';
const output = document.createElement('pre');
const host = document.createElement('main');
const canvas = document.createElement('canvas');
canvas.id = 'lcd-source';
canvas.style.cssText = 'width:720px;height:480px;display:block;image-rendering:pixelated';
host.append(canvas, output);
document.body.append(host);
const viewer = new LCDViewer(host, canvas);
const config: EngineConfig = {
  firmwareId: 'x70-110',
  bundle: '/__wasm-camera/',
  sceneInput: false,
  execution: { mode: 'replay', clockSeconds2000: 0 },
};
const errors: string[] = [];
const engine = createEngine((error) => errors.push(String(error)));
window.cameraEngine = engine;
engine.frames((frame) => viewer.frame(frame));
const result: Record<string, unknown> = {};
function check(value: unknown, message: string): asserts value {
  if (!value) throw Error(message);
}
function dmpDiagnostics(states: unknown[]): { rasterizer: string; gpuLists?: number } | undefined {
  for (const state of states) {
    if (state === null || typeof state !== 'object' || !('gles' in state)) continue;
    const gles = state.gles;
    if (
      gles !== null &&
      typeof gles === 'object' &&
      'host_rasterizer' in gles &&
      typeof gles.host_rasterizer === 'string'
    )
      return {
        rasterizer: gles.host_rasterizer,
        ...('gpu_lists' in gles && typeof gles.gpu_lists === 'number'
          ? { gpuLists: gles.gpu_lists }
          : {}),
      };
  }
  return undefined;
}
async function pixels() {
  const data = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height).data;
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', data))]
    .map((n) => n.toString(16).padStart(2, '0'))
    .join('');
}
async function advance(ticks = 6_000_000) {
  await engine.replay([{ action: 'advance', ticks }]);
}
try {
  const manifestResponse = await fetch('/__wasm-camera/wasm.json');
  check(manifestResponse.ok, 'Camera manifest unavailable.');
  const manifest = (await manifestResponse.json()) as {
    session: { firmwareId: string; clockSeconds2000: number };
    dmpCpuRenderer?: boolean;
  };
  const expectedRasterizer = manifest.dmpCpuRenderer === false ? 'WebGPU' : 'CPU';
  config.firmwareId = manifest.session.firmwareId;
  config.execution = { mode: 'replay', clockSeconds2000: manifest.session.clockSeconds2000 };
  const response = await fetch('/__wasm-camera/firmware.dat');
  check(response.ok, 'DAT unavailable.');
  const started = performance.now();
  const dat = await response.arrayBuffer();
  result.datBytes = dat.byteLength;
  result.datSha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', dat))]
    .map((n) => n.toString(16).padStart(2, '0'))
    .join('');
  const inputs = await engine.boot(dat, config, (message) => {
    output.textContent = message;
  });
  result.bootSeconds = (performance.now() - started) / 1000;
  result.controls = {
    supported: inputs.filter((input) => input.supported).length,
    unavailable: inputs.filter((input) => !input.supported).map((input) => input.id),
  };
  check(canvas.width === 720 && canvas.height === 480, 'LCD dimensions differ.');
  result.rec = await pixels();
  const dmp = dmpDiagnostics((await engine.deviceStates('dmp-smaph-h')).states);
  check(dmp !== undefined, 'DMP renderer state is unavailable.');
  check(
    dmp.rasterizer === expectedRasterizer,
    `Expected the ${expectedRasterizer} DMP renderer, got ${dmp.rasterizer}.`,
  );
  if (expectedRasterizer === 'WebGPU')
    check((dmp.gpuLists ?? 0) > 0, 'WebGPU did not complete a DMP command list.');
  result.execution = {
    profile: await engine.profile(0),
    fingerprint: await engine.fingerprint(),
    dmpRasterizer: dmp.rasterizer,
    dmpGpuLists: dmp.gpuLists ?? 0,
    frame: { width: canvas.width, height: canvas.height, sha256: result.rec },
  };
  check(
    inputs.some((input) => input.id === 'menu_ok_button' && input.supported),
    'Generated native controls are unavailable.',
  );
  const point = await engine.checkpoint(),
    fingerprint = await engine.fingerprint();
  await engine.press('menu_ok_button');
  await engine.release('menu_ok_button');
  await advance();
  result.menu = await pixels();
  check(result.menu !== result.rec, 'MENU did not change guest LCD pixels.');
  await engine.restoreCheckpoint(point.id);
  check(
    (await engine.fingerprint()).sha256 === fingerprint.sha256,
    'Native checkpoint fingerprint differs after restore.',
  );
  check((await pixels()) === result.rec, 'Restored LCD pixels differ.');
  await engine.setPosition('shutter_speed_dial', '500');
  await advance();
  result.dial = await pixels();
  check(result.dial !== result.rec, 'Shutter dial did not change LCD pixels.');
  await engine.setScene({
    width: 2,
    height: 2,
    rgb: new Uint8Array(Array(4).fill([255, 0, 0]).flat()),
  });
  await advance(24_000_000);
  result.scene = await pixels();
  check(result.scene !== result.dial, 'Scene input did not change guest LCD pixels.');
  result.card = await engine.cardTree();
  check(
    (result.card as { supported: boolean; present: boolean }).supported &&
      (result.card as { present: boolean }).present,
    'Card unavailable.',
  );
  await engine.insertCard(false);
  check(!(await engine.cardState()).present, 'Card eject failed.');
  await engine.insertCard(true);
  check((await engine.cardState()).present, 'Card insertion failed.');
  const log = await engine.history();
  result.history = { entries: log.entries, bytes: log.log.length };
  check(log.entries > 0 && log.log.length > 0, 'Native history is empty.');
  await engine.rollback(10);
  result.undo = await engine.profile(0);
  check(
    (result.undo as { ticks: number }).ticks === 30_000_000,
    'Early Undo did not restore the oldest native checkpoint.',
  );
  check((await pixels()) === result.rec, 'Undo did not restore REC pixels.');
  check(errors.length === 0, errors.join('\n'));
  result.errors = errors;
  document.body.dataset.proof = 'passed';
} catch (error) {
  result.error = String(error);
  result.errors = errors;
  document.body.dataset.proof = 'failed';
}
output.textContent = JSON.stringify(result, null, 2);
