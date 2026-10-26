import { createEngine } from '../src/engine';
import type { EngineConfig } from '@fujimod/emulator-ui/engine/api';
import { crc32 } from '@fujimod/emulator-ui/archive';
import { faultFrom } from '@fujimod/emulator-ui/engine/fault';

const result = document.createElement('pre');
document.body.append(result);
const config: EngineConfig = {
  firmwareId: 'generated-session',
  bundle: '/runtime/wasm-session-proof/',
  execution: { mode: 'replay' },
  sceneInput: false,
};
function check(value: unknown, message: string): asserts value {
  if (!value) throw Error(message);
}
function finish(value: unknown, passed: boolean) {
  result.textContent = JSON.stringify(value, null, 2);
  document.body.dataset.proof = passed ? 'passed' : 'failed';
}
async function snapshotMember(report: Blob) {
  const buffer = await report.arrayBuffer(),
    view = new DataView(buffer);
  for (let offset = 0; offset + 30 <= buffer.byteLength;) {
    check(
      view.getUint32(offset, true) === 0x04034b50 && view.getUint16(offset + 8, true) === 0,
      'Unexpected report ZIP member.',
    );
    const length = view.getUint32(offset + 18, true),
      nameLength = view.getUint16(offset + 26, true),
      extra = view.getUint16(offset + 28, true);
    const name = new TextDecoder().decode(new Uint8Array(buffer, offset + 30, nameLength));
    const begin = offset + 30 + nameLength + extra;
    const bytes = new Uint8Array(buffer.slice(begin, begin + length));
    check(
      bytes.length === length && crc32(bytes) === view.getUint32(offset + 14, true),
      'Report member CRC/length differs.',
    );
    if (name === 'snapshot.emusnap') return bytes;
    offset = begin + length;
  }
  throw Error('Report has no snapshot.emusnap.');
}
async function load(bytes: Uint8Array) {
  const engine = createEngine(() => {});
  try {
    await engine.loadSnapshot(bytes, config, (message) => {
      result.textContent = message;
    });
    const restored = await engine.fingerprint();
    const initial = await engine.profile(1);
    check(initial.ticks === 13, 'Snapshot lost the guest execution boundary.');
    const card = await engine.readCardFile('/DCIM/100_FUJI/A.JPG');
    check(new TextDecoder().decode(card) === 'camera bytes', 'Restored card data differs.');
    await engine.replay([{ action: 'advance', ticks: 687 }]);
    return {
      restored,
      continued: await engine.fingerprint(),
      ticks: (await engine.profile(1)).ticks,
    };
  } finally {
    engine.dispose();
  }
}
if (new URL(location.href).searchParams.has('load')) {
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = '.emusnap';
  input.ariaLabel = 'Load snapshot';
  document.body.prepend(input);
  input.addEventListener('change', () => {
    const file = input.files?.[0];
    if (file)
      void file
        .arrayBuffer()
        .then((bytes) => load(new Uint8Array(bytes)))
        .then(
          (value) => finish(value, true),
          (error: unknown) => finish({ error: String(error), fault: faultFrom(error) }, false),
        );
  });
} else {
  const source = createEngine(() => {});
  try {
    const dat = await (await fetch('/runtime/wasm-session-proof/program.dat')).arrayBuffer();
    await source.boot(dat, config, () => {});
    await source.setScene({ width: 1, height: 1, rgb: new Uint8Array([91, 37, 13]) });
    await source.replay([{ action: 'advance', ticks: 13 }]);
    const before = await source.fingerprint();
    const direct = await source.exportSnapshot();
    check(direct.length > 0, 'Explicit export is empty.');
    const bytes = await snapshotMember(await source.errorReport());
    check(
      (await source.fingerprint()).sha256 === before.sha256,
      'Export changed the source machine.',
    );
    await source.replay([{ action: 'advance', ticks: 687 }]);
    const expected = await source.fingerprint();
    source.dispose();
    const restored = await load(bytes);
    check(restored.restored.sha256 === before.sha256, 'Fresh worker restore fingerprint differs.');
    check(restored.continued.sha256 === expected.sha256, 'Fresh worker continuation differs.');
    const link = document.createElement('a');
    link.download = 'snapshot.emusnap';
    link.textContent = 'Download snapshot';
    link.href = URL.createObjectURL(new Blob([bytes]));
    document.body.prepend(link);
    finish({ before, expected, ...restored, bytes: bytes.length }, true);
  } catch (error) {
    finish({ error: String(error), fault: faultFrom(error) }, false);
  } finally {
    source.dispose();
  }
}
