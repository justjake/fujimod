import { createEngine } from '../src/engine';
import { EngineError } from '@fujimod/emulator-ui/engine/fault';
import type { CardEntry } from 'fujimod/emulator';
import { LCDViewer } from '@fujimod/emulator-ui/viewer/lcd';
import { pixelQuality } from './pixel-quality';

const canvas = document.createElement('canvas'),
  output = document.createElement('pre');
canvas.id = 'lcd-source';
canvas.style.cssText = 'width:720px;height:480px;display:block;image-rendering:pixelated';
document.body.append(canvas, output);
const viewer = new LCDViewer(document.body, canvas);
const result: Record<string, unknown> = { phases: [], firstUse: [] };
const phases = result.phases as Record<string, unknown>[];
const firstUse = result.firstUse as Record<string, unknown>[];
const photos: { path: string; bytes: number; sha256: string; url: string }[] = [];
result.photos = photos;
const errors: string[] = [];
const exposureControls = [
  { id: 'shutter_speed_dial', position: 'A' },
  { id: 'aperture_ring', position: 'A' },
  { id: 'exposure_compensation_dial', position: '0' },
];
result.exposureControls = exposureControls;
const engine = createEngine((error) => errors.push(String(error)));
window.cameraEngine = engine;
let delivered = 0;
engine.frames((frame) => {
  viewer.frame(frame);
  delivered++;
});
engine.metrics((metrics) => {
  window.cameraPerformance = metrics;
});
const show = () => {
  output.textContent = JSON.stringify(result, null, 2);
};
const check = (value: unknown, message: string) => {
  if (!value) throw Error(message);
};
type Profile = Record<string, unknown> & { ticks: string; observations: Record<string, number[]> };
const profile = async () => (await engine.profile(0)) as Profile;
const u32 = (bytes: number[]) => new DataView(Uint8Array.from(bytes).buffer).getUint32(0, true);
const ready = (p: Profile) =>
  u32(p.observations.mode!) === 3 && !u32(p.observations.pending!) && !u32(p.observations.busy!);
const advance = (ticks = 1_200_000) => engine.replay([{ action: 'advance', ticks }]);
const tap = async (id: string) => {
  await engine.press(id);
  await engine.release(id);
};
const hash = async (bytes: BufferSource) =>
  [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))]
    .map((n) => n.toString(16).padStart(2, '0'))
    .join('');
const header = () => hash(canvas.getContext('2d')!.getImageData(0, 0, 720, 48).data);
const files = (entries: CardEntry[]): CardEntry[] =>
  entries.flatMap((e) => (e.kind === 'directory' ? files(e.children ?? []) : [e]));
async function until(test: (p: Profile) => boolean | Promise<boolean>, maximum = 120_000_000) {
  const start = BigInt((await profile()).ticks);
  for (;;) {
    const p = await profile();
    if (await test(p)) return p;
    if (BigInt(p.ticks) - start >= BigInt(maximum))
      throw Error('Guest completion deadline exceeded.');
    await advance();
  }
}
async function phase(
  name: string,
  operation: () => Promise<void>,
  stage: 'cold' | 'first' | 'warm',
) {
  const cold = stage === 'cold';
  const before = cold ? undefined : await profile();
  const wall = performance.now();
  let error: string | undefined;
  try {
    await operation();
  } catch (failure) {
    error =
      failure instanceof EngineError
        ? `${failure.message} ${JSON.stringify(failure.fault.details)}`
        : String(failure);
  }
  const end = performance.now();
  const after = await profile().catch((failure) => {
    error ??= String(failure);
    return undefined;
  });
  const guestSeconds = after
    ? Number(BigInt(after.ticks) - BigInt(before?.ticks ?? '0')) / 12_000_000
    : null;
  const measurement = {
    name,
    stage,
    guestSeconds,
    wallSeconds: (end - wall) / 1000,
    speed: guestSeconds === null ? null : guestSeconds / ((end - wall) / 1000),
    completed: !error,
    error,
    before,
    after,
  };
  if (cold) result.coldBoot = measurement;
  else (stage === 'warm' ? phases : firstUse).push(measurement);
  console.log(JSON.stringify({ phase: name, ...measurement, before: undefined, after: undefined }));
  show();
  if (!after) throw Error(error ?? 'Native profile unavailable.');
}
async function repeated(name: string, operation: () => Promise<void>) {
  await phase(name, operation, 'first');
  await phase(name, operation, 'warm');
}
try {
  // The caller-supplied DAT is selected before power-on. Runtime loading, verification,
  // construction, boot execution, initial checkpoint and first delivered LCD are timed.
  const dat = await (await fetch('/__wasm-camera/firmware.dat')).arrayBuffer();
  await phase(
    'cold boot to REC',
    async () => {
      const manifestResponse = await fetch('/__wasm-camera/wasm.json');
      check(manifestResponse.ok, 'Camera manifest unavailable.');
      const manifest = (await manifestResponse.json()) as {
        session: { firmwareId: string; clockSeconds2000: number };
      };
      const config = {
        firmwareId: manifest.session.firmwareId,
        bundle: '/__wasm-camera/',
        sceneInput: true,
        execution: { mode: 'replay' as const, clockSeconds2000: manifest.session.clockSeconds2000 },
      };
      engine.prepare(config);
      await engine.boot(dat, config, () => {});
      await until(ready);
      check(canvas.width === 720 && canvas.height === 480, 'REC LCD was not delivered.');
    },
    'cold',
  );
  for (const setting of exposureControls) await engine.setPosition(setting.id, setting.position);
  await advance(12_000_000);
  await repeated('REC live view', async () => {
    const frames = delivered;
    await advance(120_000_000);
    result.recDeliveredFrames = delivered - frames;
    check(delivered - frames >= 100, 'REC lost LCD presentation frames.');
    check(ready(await profile()), 'REC readiness differs.');
  });
  await repeated('MENU navigation', async () => {
    const rec = await header();
    await tap('menu_ok_button');
    await until(async () => (await header()) !== rec, 60_000_000);
    await tap('selector.down');
    await advance(3_600_000);
    await tap('disp_back_button');
    await until(async (p) => ready(p) && (await header()) === rec, 60_000_000);
  });
  // This phase requires the stock AF completion notification or a stable REC21=8.
  await engine.setPosition('focus_mode_selector', 'S');
  await advance(6_000_000);
  for (const stage of ['first', 'warm'] as const) {
    await phase(
      'AF-S half-press',
      async () => {
        let last = u32((await profile()).observations.afUI!),
          changed = false,
          stable: bigint | undefined;
        await engine.press('shutter_button', 'half');
        await until((p) => {
          const notification = u32(p.observations.afUI!);
          changed ||= notification !== last;
          last = notification;
          if (changed && [0x816, 0x829].includes(notification)) return true;
          const machine = new DataView(Uint8Array.from(p.observations.rec21!).buffer).getUint16(
            0,
            true,
          );
          if (machine !== 8) stable = undefined;
          else if (stable === undefined) stable = BigInt(p.ticks);
          return stable !== undefined && BigInt(p.ticks) - stable >= 1_200_000n;
        });
      },
      stage,
    );
    await engine.release('shutter_button');
    await advance(6_000_000);
  }
  // An incomplete AF phase must not poison the independent capture/Playback measurements.
  await engine.powerCycle([], () => {});
  check(ready(await profile()), 'Capture cold-start prerequisite failed.');
  // Capture is independently qualified with the physical manual-focus position.
  await engine.setPosition('focus_mode_selector', 'M');
  for (const setting of exposureControls) await engine.setPosition(setting.id, setting.position);
  await advance(6_000_000);
  await engine.setScene(null);
  let jpeg: Uint8Array<ArrayBuffer> | undefined;
  const capture = async () => {
    const seen = new Set(files((await engine.cardTree()).entries).map((f) => f.path));
    let path: string | undefined;
    await engine.press('shutter_button', 'half');
    await engine.press('shutter_button', 'full');
    await until(async () => {
      if ((await engine.cardState()).busy) return false;
      try {
        const photo = files((await engine.cardTree()).entries).find(
          (f) => /\.JPG$/i.test(f.path) && !seen.has(f.path),
        );
        if (!photo) return false;
        jpeg = (await engine.readCardFile(photo.path)).slice();
        path = photo.path;
        return (
          jpeg.length > 100000 &&
          jpeg[0] === 255 &&
          jpeg[1] === 216 &&
          jpeg.at(-2) === 255 &&
          jpeg.at(-1) === 217
        );
      } catch (error) {
        if (!/busy|chain|FAT|directory|extent|sector|cluster|file|revision/i.test(String(error)))
          throw error;
        return false;
      }
    }, 180_000_000);
    result.jpeg = { bytes: jpeg!.length, sha256: await hash(jpeg!.slice().buffer), focus: 'M' };
    check(path, 'Closed JPEG path unavailable.');
    photos.push({
      path: path!,
      bytes: jpeg!.length,
      sha256: await hash(jpeg!.slice().buffer),
      url: URL.createObjectURL(new Blob([jpeg!.slice().buffer])),
    });
  };
  const releaseCapture = async () => {
    await engine.release('shutter_button');
    await advance(6_000_000);
  };
  const playback = async () => {
    check(jpeg, 'Capture prerequisite failed.');
    const image = await createImageBitmap(new Blob([jpeg!.slice().buffer]));
    const reference = new OffscreenCanvas(720, 480).getContext('2d')!;
    reference.drawImage(image, 0, 0, 720, 480);
    image.close();
    const expected = reference.getImageData(0, 0, 720, 480).data;
    await tap('playback_button');
    await until((p) => {
      if (u32(p.observations.mode!) !== 4 || u32(p.observations.busy!)) return false;
      const q = pixelQuality(
        expected,
        canvas.getContext('2d')!.getImageData(0, 0, 720, 480).data,
        720,
        { left: 24, top: 64, right: 696, bottom: 416 },
      );
      result.playbackQuality = q;
      return Math.max(...q.mae) <= 3 && Math.min(...q.correlation) >= 0.98;
    }, 240_000_000);
  };
  // Keep the same native machine and JIT between each first-use/warm pair.
  // First Playback is exercised before the second capture, so both operating
  // phases are repeated without a power cycle discarding their compiled code.
  await phase('capture to closed JPEG', capture, 'first');
  await releaseCapture();
  await phase('Playback', playback, 'first');
  await engine.press('shutter_button', 'half');
  await until(ready, 60_000_000);
  await engine.release('shutter_button');
  await advance(6_000_000);
  await phase('capture to closed JPEG', capture, 'warm');
  await releaseCapture();
  await phase('Playback', playback, 'warm');
  const jit = (await profile()).jit as {
    enabled: boolean;
    compiledExecutions: string;
    failures: number;
  };
  result.jit = jit;
  check(
    jit.enabled && BigInt(jit.compiledExecutions) > 0n && jit.failures === 0,
    'Default JIT execution failed.',
  );
  result.errors = errors;
  result.passed =
    phases.length === 5 &&
    phases.every((p) => p.completed && Number(p.speed) >= 1) &&
    !errors.length;
  document.body.dataset.proof = result.passed ? 'passed' : 'failed';
} catch (error) {
  result.error = String(error);
  document.body.dataset.proof = 'failed';
} finally {
  show();
  engine.dispose();
}
