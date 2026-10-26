import type { DetentSound } from '@fujimod/emulator-ui/feedback/detent-sound';
import type { Engine } from '@fujimod/emulator-ui/engine/api';
declare global {
  interface Window {
    requests: { id: string; value: string | number }[];
    clicks: number;
    clickTimbres: number[];
    vibrations: (number | number[])[];
    haptics: boolean;
    panel: ControlSession;
    fixtureReady: boolean;
  }
}
import { ControlSession, Controls } from '@fujimod/emulator-ui/ui/controls';
import { render, createComponent } from '@solidjs/web';
import { flush } from 'solid-js';
import panelData from '../../devices/x70/panel.json';
window.requests = [];
window.clicks = 0;
window.clickTimbres = [];
window.vibrations = [];
window.haptics = false;
Object.defineProperty(navigator, 'vibrate', {
  configurable: true,
  value: (duration: number | number[]) => {
    window.vibrations.push(duration);
    return window.haptics;
  },
});
const createBufferSource = AudioContext.prototype.createBufferSource;
const buffers = new Map<AudioBuffer | null, number>();
AudioContext.prototype.createBufferSource = function (...args) {
  const source = createBufferSource.apply(this, args),
    start = source.start.bind(source);
  source.start = (...args) => {
    if (!buffers.has(source.buffer)) buffers.set(source.buffer, buffers.size);
    window.clicks++;
    window.clickTimbres.push(buffers.get(source.buffer)!);
    start(...args);
  };
  return source;
};
const viewer = { pressFeedback() {} };
window.panel = new ControlSession(
  viewer,
  (error) => {
    throw error;
  },
  async () => {},
);
window.panel.render(
  [
    {
      id: 'aperture_ring',
      label: 'Aperture',
      kind: 'ring',
      supported: true,
      positions: ['2.8', '4', '5.6', '8', '11', '16'].map((id) => ({ id, label: id })),
      initial: '5.6',
    },
    {
      id: 'shutter_speed_dial',
      label: 'Speed',
      kind: 'dial',
      supported: true,
      positions: ['30', '60', '125', '250', '500', '1000'].map((id) => ({ id, label: id })),
      initial: '125',
    },
    { id: 'control_ring', label: 'Command', kind: 'dial', supported: true, relative: true },
  ],
  {
    rotate: async (id: string, value: number) => {
      window.requests.push({ id, value });
    },
    setPosition: async (id: string, value: string) => {
      window.requests.push({ id, value });
    },
  } as unknown as Engine,
  {
    schema: 1,
    body: 'x70',
    detentSounds: panelData.detentSounds as Record<string, DetentSound>,
    layout: [
      { kind: 'control', id: 'aperture_ring', variant: 'dial' },
      { kind: 'control', id: 'shutter_speed_dial', variant: 'dial' },
      { kind: 'control', id: 'control_ring', variant: 'ticks' },
    ],
    combos: [],
  },
);
render(
  () => createComponent(Controls, { session: window.panel }),
  document.querySelector('#controls')!,
);
flush();
window.fixtureReady = true;
