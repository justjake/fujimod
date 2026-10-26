import { render } from '@solidjs/web';
import { createSignal, flush } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
import { ToolDeck } from './ToolDeck';

test('the splash explains service mode and offers the two ways in', () => {
  const host = document.createElement('div');
  const onServiceMode = vi.fn(),
    onConnect = vi.fn();
  const dispose = render(
    () => (
      <ToolDeck
        usbSupported
        savedCameras={[]}
        simulationAvailable={false}
        onServiceMode={onServiceMode}
        onConnect={onConnect}
        onSimulated={() => {}}
      />
    ),
    host,
  );
  try {
    flush();
    expect(host.querySelector('.td-enter h2')!.textContent).toBe('Enter service mode');
    expect(host.querySelector('.td-enter')!.textContent).toContain('restores factory settings');
    host.querySelector<HTMLButtonElement>('.td-enter button')!.click();
    expect(onServiceMode).toHaveBeenCalledOnce();
    host.querySelector<HTMLButtonElement>('button[aria-label="Connect camera"]')!.click();
    expect(onConnect).toHaveBeenCalledOnce();
    const simulated = host.querySelector('.td-simulated')!;
    expect(simulated.textContent).toContain('Simulated USB isn’t in this build yet.');
    expect(simulated.querySelector('.td-empty a')!.getAttribute('href')).toBe('../');
    expect(host.querySelector('.td-unsupported')).toBeNull();
    expect(host.querySelector('h1')!.className).toBe('visually-hidden');
    expect(host.querySelector<HTMLAnchorElement>('.td-footer-link')!.getAttribute('href')).toBe(
      '../bug/',
    );
  } finally {
    dispose();
  }
});

test('without WebUSB the page says why and the camera tile is unavailable', () => {
  const host = document.createElement('div');
  const onConnect = vi.fn();
  const dispose = render(
    () => (
      <ToolDeck
        usbSupported={false}
        savedCameras={[]}
        simulationAvailable={false}
        onServiceMode={() => {}}
        onConnect={onConnect}
        onSimulated={() => {}}
      />
    ),
    host,
  );
  try {
    flush();
    const tile = host.querySelector<HTMLButtonElement>('button[aria-label="Connect camera"]')!;
    expect(tile.disabled).toBe(true);
    expect(host.querySelector('.td-unsupported')!.textContent).toBe(
      'This browser can’t connect to USB. Use Chrome or Edge.',
    );
    tile.click();
    expect(onConnect).not.toHaveBeenCalled();
  } finally {
    dispose();
  }
});

test('the simulated camera tile lists every saved camera and starts the chosen one', () => {
  const host = document.createElement('div');
  const onSimulated = vi.fn();
  const [available, setAvailable] = createSignal(false);
  const dispose = render(
    () => (
      <ToolDeck
        usbSupported
        savedCameras={[
          { id: 'a', name: 'My X70' },
          { id: 'b', name: 'Second X70' },
        ]}
        simulationAvailable={available()}
        onServiceMode={() => {}}
        onConnect={() => {}}
        onSimulated={onSimulated}
      />
    ),
    host,
  );
  try {
    flush();
    const choices = [...host.querySelectorAll<HTMLButtonElement>('.td-saved-camera')];
    expect(choices.map((choice) => choice.textContent)).toEqual(['My X70', 'Second X70']);
    expect(choices.every((choice) => choice.disabled)).toBe(true);
    setAvailable(true);
    flush();
    expect(host.querySelector('.td-simulated .td-lock')).toBeNull();
    choices[1].click();
    expect(onSimulated).toHaveBeenCalledExactlyOnceWith('b');
  } finally {
    dispose();
  }
});
