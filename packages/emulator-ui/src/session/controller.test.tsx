import { createStore, flush } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
import type { PhysicalControls } from 'fujimod';
import type { ControlInput } from 'fujimod/emulator';
import x70 from '../../../../catalog/bodies/x70/controls.json';
import type { Engine } from '../engine/api';
import { sessionState } from './state';
import { startSession } from './controller';

test('power-off on boot remains visible, a power-cycle wakes it, and reset drops stale state', async () => {
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
  const [state, setState] = createStore(sessionState());
  const host = document.createElement('div'),
    lcd = document.createElement('canvas');
  host.append(lcd);
  document.body.append(host);
  const inputs: ControlInput[] = [
    {
      id: 'power_switch',
      label: 'Power',
      kind: 'switch',
      supported: true,
      positions: [
        { id: 'OFF', label: 'OFF' },
        { id: 'ON', label: 'ON' },
      ],
      initial: 'ON',
    },
  ];
  let powerChanged: ((on: boolean) => void) | undefined;
  const powerCycle = vi.fn<Engine['powerCycle']>(async () => {
    powerChanged?.(true);
  });
  const engine = {
    boot: async () => inputs,
    metrics: () => () => {},
    connections: () => () => {},
    cameraSaves: () => () => {},
    frames: () => () => {},
    powerChanges(listener) {
      powerChanged = listener;
      listener(false);
      return () => {};
    },
    powerCycle,
    dispose: vi.fn(),
  } satisfies Partial<Engine>;
  const runtime = startSession(
    state,
    setState,
    { viewer: host, lcd },
    {
      createEngine: async () => engine as unknown as Engine,
      identity: () => ({ build: 'test', modBuild: 'test', canRestart: true, sceneInput: false }),
      simulatorBuild: 'test',
      restart: async () => {},
      fault: vi.fn(),
      ready: vi.fn(),
      cameraSaved: vi.fn(),
    },
  );
  try {
    await runtime.boot(
      new ArrayBuffer(0),
      {
        schema: 1,
        body: 'x70',
        combos: [],
        layout: [{ kind: 'control', id: 'power_switch', variant: 'segment', operation: 'power' }],
      },
      x70 as unknown as PhysicalControls,
      () => true,
      () => ({
        config: { firmwareId: 'test', bundle: 'https://example.invalid/', sceneInput: false },
      }),
    );
    flush();
    expect(state.poweredOn).toBe(false);
    expect(state.live).toBe('OFF');
    expect(state.message).toBe('Camera is powered off');
    expect(lcd.style.visibility).toBe('hidden');
    expect(runtime.panel.positions.get('power_switch')).toBe('ON');
    runtime.panel.positions.set('power_switch', 'OFF');
    await runtime.turnOn();
    flush();
    expect(powerCycle).toHaveBeenCalledWith(
      [{ id: 'power_switch', position: 'ON' }],
      expect.any(Function),
    );
    expect(state.poweredOn).toBe(true);
    expect(state.live).toBe('LIVE');
    expect(state.message).toBe('Camera ready');
    expect(lcd.style.visibility).toBe('');
    expect(runtime.panel.ready).toBe(true);
    expect(runtime.panel.powered).toBe(true);
    runtime.reset();
    flush();
    powerChanged?.(false);
    flush();
    expect(state.poweredOn).toBeUndefined();
  } finally {
    runtime.dispose();
    host.remove();
    vi.unstubAllGlobals();
  }
});
