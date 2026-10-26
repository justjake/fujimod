import { createEngine } from '../src/engine';
import type { Engine, EngineConfig } from '@fujimod/emulator-ui/engine/api';
import type { CardState } from 'fujimod/emulator';
import { CameraStore } from '../src/cameras/store';
import { engineReport } from './wasm-report';
export async function proveSession(base: EngineConfig) {
  const config = {
    ...base,
    firmwareId: 'generated-session',
    bundle: '/runtime/wasm-session-proof/',
    camera: undefined as EngineConfig['camera'],
    execution: { mode: 'replay' as const },
  };
  const errors: string[] = [],
    cards: CardState[] = [],
    scenes: number[] = [],
    connections: unknown[] = [];
  const engine: Engine = createEngine((error) => errors.push(String(error)));
  engine.cardChanges((state) => cards.push(state));
  engine.connections((rows) => connections.push(rows));
  const check = (value: unknown, message: string) => {
    if (!value) throw Error(message);
  };
  async function sourceFrames() {
    const report = await engineReport(await engine.errorReport());
    const bus = report.bus as { devices: { kind: string; state: { frames: number } }[] };
    const device = bus.devices.find((row) => row.kind === 'external-image-frames');
    check(device, 'Missing scene device diagnostics.');
    return device!.state.frames;
  }
  try {
    const dat = await (await fetch('/runtime/wasm-session-proof/program.dat')).arrayBuffer();
    console.log('Session fixture: create');
    const record = await new CameraStore().create(
      new File([dat], 'program.dat'),
      { id: config.firmwareId, model: 'Generated', version: '1', sha256: 'generated' },
      false,
    );
    config.camera = { id: record.id, revision: 0, modEnabled: false };
    console.log('Session fixture: boot');
    const inputs = await engine.boot(dat, config, () => {});
    check(inputs.length === 0, 'Unresolved physical bindings must not be advertised.');
    const first = await engine.cardState(),
      tree = await engine.cardTree();
    check(
      first.supported && first.present && tree.entries[0]?.path === '/DCIM',
      'Card state/tree differs.',
    );
    const bytes = await engine.readCardFile('/DCIM/100_FUJI/A.JPG', {
      revision: tree.revision!,
      maxBytes: 100,
    });
    check(new TextDecoder().decode(bytes) === 'camera bytes', 'Card read bytes differ.');
    const stale = await engine
      .readCardFile('/DCIM/100_FUJI/A.JPG', { revision: tree.revision! + 1 })
      .then(
        () => '',
        (error) => String(error),
      );
    check(stale.includes('revision'), 'Stale revision was accepted.');
    const limit = await engine.readCardFile('/DCIM/100_FUJI/A.JPG', { maxBytes: 1 }).then(
      () => '',
      (error) => String(error),
    );
    check(limit.length, 'Card read limit was ignored.');
    const before = await engineReport(await engine.errorReport());
    await engine.cardState();
    await engine.cardTree();
    await engine.profile(2);
    check(
      JSON.stringify(before) === JSON.stringify(await engineReport(await engine.errorReport())),
      'Host reads changed guest state.',
    );
    scenes.push(await sourceFrames());
    await engine.setScene({
      width: 2,
      height: 2,
      rgb: new Uint8Array(Array(4).fill([255, 0, 0]).flat()),
    });
    await engine.replay([{ action: 'advance', ticks: 10 }]);
    scenes.push(await sourceFrames());
    console.log('Session fixture: default scene');
    await engine.setScene(null);
    await engine.replay([{ action: 'advance', ticks: 10 }]);
    scenes.push(await sourceFrames());
    check(
      JSON.stringify(scenes) === '[0,1,2]',
      'Scene source did not advance at its guest clock edges.',
    );
    await engine.insertCard(false);
    check(!(await engine.cardState()).present, 'Eject did not change contacts.');
    const absent = await engine.readCardFile('/DCIM/100_FUJI/A.JPG').then(
      () => '',
      (error) => String(error),
    );
    check(/absent|inserted|present/.test(absent), 'Absent-card read was accepted.');
    await engine.insertCard(true);
    await engine.formatCard(() => {});
    const formatted = await engine.cardTree();
    check(formatted.entries.length === 0, 'Format did not replace the card filesystem.');
    const pending: [string, () => Promise<unknown>][] = [
      ['press', () => engine.press('menu')],
      ['release', () => engine.release('menu')],
      ['set_position', () => engine.setPosition('dial', 'unknown')],
    ];
    const refusals: Record<string, string> = {};
    for (const [name, run] of pending) {
      refusals[name] = await run().then(
        () => '',
        (error) => String(error),
      );
      check(refusals[name]!.length > 0, `${name} accepted an unresolved physical binding.`);
    }
    console.log('Session fixture: save');
    await engine.saveCamera();
    console.log('Session fixture: OFF');
    await engine.power(false);
    await engine.replay([{ action: 'advance', ticks: 10 }]);
    check((await engine.profile(2)).ticks === 0, 'OFF executed guest instructions.');
    console.log('Session fixture: ON');
    await engine.power(true);
    console.log('Session fixture: cycle');
    await engine.powerCycle([], () => {});
    console.log('Session fixture: merge');
    await engine.restartWithCardFiles(
      [{ path: '/NEW.TXT', bytes: new Uint8Array([1, 2, 3]) }],
      () => {},
    );
    check(
      JSON.stringify([...(await engine.readCardFile('/NEW.TXT'))]) === '[1,2,3]',
      'Cold card merge differs.',
    );
    const profile = await engine.profile(2);
    check(
      profile.failed === false && profile.ticks === 0,
      'Cold lifecycle did not reset guest execution state.',
    );
    check(errors.length === 0, 'An operation refusal became an execution fault.');
    check(
      cards.some((state) => !state.present) && cards.at(-1)?.present,
      'Card change listener missed transitions.',
    );
    check(connections.length > 0, 'Connection state was not delivered.');
    return { scenes, cardRevision: formatted.revision, profile, refusals, errors };
  } finally {
    engine.dispose();
  }
}
