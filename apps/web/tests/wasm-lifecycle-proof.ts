import { createEngine } from '../src/engine';
import type { Engine, EngineConfig } from '@fujimod/emulator-ui/engine/api';
import type { CardEntry } from 'fujimod/emulator';
import { CameraStore } from '../src/cameras/store';
import { LCDViewer } from '@fujimod/emulator-ui/viewer/lcd';
import { engineReport } from './wasm-report';
import { pixelQuality } from './pixel-quality';
const host = document.createElement('main'),
  canvas = document.createElement('canvas'),
  output = document.createElement('pre');
canvas.id = 'lcd-source';
canvas.style.cssText = 'width:720px;height:480px;display:block;image-rendering:pixelated';
host.append(canvas, output);
document.body.append(host);
const viewer = new LCDViewer(host, canvas),
  store = new CameraStore(),
  errors: string[] = [],
  saves: unknown[] = [],
  powers: boolean[] = [];
const result: Record<string, unknown> = {};
let engine!: Engine;
const check = (value: unknown, message: string) => {
  if (!value) throw Error(message);
};
function show(stage: string) {
  result.stage = stage;
  console.log(stage);
  output.textContent = JSON.stringify(result, null, 2);
}
function attach() {
  engine = createEngine((error) => errors.push(String(error)));
  window.cameraEngine = engine;
  engine.frames((frame) => viewer.frame(frame));
  engine.cameraSaves((event) => saves.push(event));
  engine.errors((fault) => {
    result.fault = fault;
  });
  engine.powerChanges((on) => {
    powers.push(on);
    viewer.powered(on);
  });
}
const hash = async (bytes: BufferSource) =>
  [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))]
    .map((n) => n.toString(16).padStart(2, '0'))
    .join('');
const pixels = () =>
  hash(canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height).data);
const advance = (ticks = 6_000_000) => engine.replay([{ action: 'advance', ticks }]);
const tap = async (id: string) => {
  await engine.press(id);
  await engine.release(id);
  await advance();
};
const files = (entries: CardEntry[]): CardEntry[] =>
  entries.flatMap((e) => (e.kind === 'directory' ? files(e.children ?? []) : [e]));
try {
  show('Loading owned camera');
  const dat = await (await fetch('/__wasm-camera/firmware.dat')).arrayBuffer();
  const record = await store.create(
    new File([dat], 'camera.dat'),
    { id: 'x70-110', model: 'X70', version: '1.10', sha256: await hash(dat) },
    false,
  );
  const config: EngineConfig = {
    firmwareId: record.firmwareId,
    bundle: '/__wasm-camera/',
    sceneInput: true,
    camera: { id: record.id, revision: record.revision, modEnabled: false },
    execution: { mode: 'replay', clockSeconds2000: 845000000 },
  };
  attach();
  engine.prepare(config);
  const inputs = await engine.boot(dat.slice(0), config, () => {});
  result.controls = {
    supported: inputs.filter((i) => i.supported).length,
    positions: inputs
      .filter((i) => i.supported)
      .reduce((n, i) => n + (i.positions?.length ?? 0), 0),
  };
  check(
    inputs.find((i) => i.id === 'exposure_compensation_dial')?.positions?.length === 19,
    'Exposure detents differ.',
  );
  check(
    inputs.find((i) => i.id === 'aperture_ring')?.positions?.length === 17,
    'Aperture detents differ.',
  );
  const rec = await pixels(),
    point = await engine.checkpoint(),
    fingerprint = await engine.fingerprint();
  show('Generated MENU and Undo');
  await tap('menu_ok_button');
  result.menu = await pixels();
  check(result.menu !== rec, 'MENU pixels did not change.');
  await engine.restoreCheckpoint(point.id);
  check((await engine.fingerprint()).sha256 === fingerprint.sha256, 'Undo fingerprint differs.');
  check((await pixels()) === rec, 'Undo pixels differ.');
  result.undo = true;
  await engine.setPosition('shutter_speed_dial', '125');
  check((await engine.undo()).shutter_speed_dial === 'A', 'Native Undo did not restore the dial.');
  await engine.setPosition('shutter_speed_dial', '125');
  await engine.setPosition('focus_mode_selector', 'M');
  await advance();

  show('Half and full shutter');
  const seen = new Set(files((await engine.cardTree()).entries).map((f) => f.path));
  await engine.setScene(null);
  await engine.press('shutter_button', 'half');
  await engine.press('shutter_button', 'full');
  await advance(36_000_000);
  await engine.release('shutter_button');
  let photo: CardEntry | undefined;
  for (let n = 0; n < 30; n++) {
    await advance(6_000_000);
    const state = await engine.cardState();
    if (state.busy) continue;
    try {
      photo = files((await engine.cardTree()).entries).find(
        (f) => /\.JPG$/i.test(f.path) && !seen.has(f.path),
      );
      if (photo) break;
    } catch (error) {
      if (!/busy|chain|FAT|directory|extent|sector|cluster|file|revision/i.test(String(error)))
        throw error;
    }
  }
  if (!photo) {
    result.card = await engine.cardTree();
    result.profile = await engine.profile(5);
  }
  check(photo, 'Shutter did not produce a photo.');
  const jpeg = await engine.readCardFile(photo!.path);
  check(
    jpeg.length > 100_000 &&
      jpeg[0] === 255 &&
      jpeg[1] === 216 &&
      jpeg.at(-2) === 255 &&
      jpeg.at(-1) === 217,
    'JPEG is incomplete.',
  );
  const image = await createImageBitmap(new Blob([jpeg.slice().buffer]));
  result.photo = {
    path: photo!.path,
    bytes: jpeg.length,
    width: image.width,
    height: image.height,
    sha256: await hash(jpeg.slice().buffer),
  };
  check(image.width > 720 && image.height > 480, 'Photo resolution differs.');
  await advance(6_000_000);
  const beforePlayback = await pixels();
  show('Playback pixels');
  await tap('playback_button');
  const reference = new OffscreenCanvas(720, 480).getContext('2d')!;
  reference.drawImage(image, 0, 0, 720, 480);
  image.close();
  const expected = reference.getImageData(0, 0, 720, 480).data;
  let quality: { mae: number[]; correlation: number[] } | undefined;
  for (let n = 0; n < 20; n++) {
    await advance(12_000_000);
    const actual = canvas.getContext('2d')!.getImageData(0, 0, 720, 480).data;
    quality = pixelQuality(expected, actual, 720, { left: 24, top: 64, right: 696, bottom: 416 });
    if (Math.max(...quality.mae) <= 3 && Math.min(...quality.correlation) >= 0.98) break;
  }
  result.playbackQuality = quality;
  check(
    quality && Math.max(...quality.mae) <= 3 && Math.min(...quality.correlation) >= 0.98,
    'Playback does not match the saved JPEG.',
  );
  result.playback = await pixels();
  check(result.playback !== beforePlayback, 'Playback did not change pixels.');
  await engine.setScene(null);
  show('Durable save');
  await engine.saveCamera();
  check(saves.length > 0, 'Save listener missed publication.');
  const durable = await store.saved(record.id);
  check(durable?.durable.byteLength, 'Durable save is empty.');
  result.durableBytes = durable!.durable.byteLength;
  result.beforePower = await engine.profile(0);
  show('Power OFF');
  await engine.power(false);
  const off = await engine.profile(2);
  await advance();
  check((await engine.profile(2)).ticks === off.ticks, 'Powered-off camera executed.');
  check(powers.at(-1) === false, 'Power listener missed OFF.');
  engine.dispose();
  const latest = (await store.list()).find((c) => c.id === record.id)!;
  attach();
  const restored = await engine.boot(
    await (await store.firmware(record.firmwareHash)).arrayBuffer(),
    { ...config, camera: { id: record.id, revision: latest.revision, modEnabled: false } },
    () => {},
  );
  check(powers.at(-1) === false, 'Saved OFF state did not restore.');
  check(
    restored.find((i) => i.id === 'shutter_speed_dial')?.initial === '125',
    'Persistent dial did not restore.',
  );
  check(
    (await hash((await engine.readCardFile(photo!.path)).slice().buffer)) ===
      (result.photo as { sha256: string }).sha256,
    'Saved JPEG differs.',
  );
  result.restoredOff = true;
  show('Power ON');
  await engine.power(true);
  check(powers.at(-1) === true, 'Power ON failed.');
  show('Held Playback cold boot');
  await engine.powerCycle([{ id: 'playback_button' }], () => {});
  check(!(await engine.profile(2)).failed, 'Held boot failed.');
  result.heldBootPixels = await pixels();
  show('Merge card files');
  await engine.restartWithCardFiles(
    [{ path: '/TEST.TXT', bytes: new TextEncoder().encode('merged media') }],
    () => {},
  );
  check(
    new TextDecoder().decode(await engine.readCardFile('/TEST.TXT')) === 'merged media',
    'Merged file differs.',
  );
  check(
    (await hash((await engine.readCardFile(photo!.path)).slice().buffer)) ===
      (result.photo as { sha256: string }).sha256,
    'Merge changed the photo.',
  );
  await engine.insertCard(false);
  check(!(await engine.cardState()).present, 'Eject failed.');
  await engine.insertCard(true);
  check((await engine.cardState()).present, 'Insert failed.');
  show('Format and report');
  await engine.formatCard(() => {});
  check(
    !files((await engine.cardTree()).entries).some(
      (f) => f.path === photo!.path || f.path === '/TEST.TXT',
    ),
    'Format retained old files.',
  );
  show('Error report');
  const blob = await engine.errorReport();
  result.reportBytes = blob.size;
  console.log('Report bytes: ' + blob.size);
  const report = await engineReport(blob);
  check(report.runtime === 'wasm' && report.failed === false, 'Error report differs.');
  result.profile = await engine.profile(5);
  check(!(result.profile as { failed: boolean }).failed, 'Native execution failed.');
  result.saves = saves;
  result.powers = powers;
  result.errors = errors;
  check(!errors.length, errors.join('\n'));
  result.passed = true;
  show('Passed');
  document.body.dataset.proof = 'passed';
} catch (error) {
  result.error = String(error);
  result.failedAt = result.stage;
  result.errors = errors;
  console.log(JSON.stringify(result));
  show('Failed');
  document.body.dataset.proof = 'failed';
}
