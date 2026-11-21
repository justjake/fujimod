import { render } from '@solidjs/web';
import { createStore, flush } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
import { PowerOff } from './PowerOff';

test('a sleeping camera exposes a real power-cycle action and removes it after wake', () => {
  const [state, setState] = createStore({ off: false });
  const turnOn = vi.fn();
  const host = document.createElement('div');
  document.body.append(host);
  const dispose = render(() => <PowerOff visible={state.off} turnOn={turnOn} />, host);
  try {
    flush();
    expect(host.querySelector('button')).toBeNull();
    setState((s) => {
      s.off = true;
    });
    flush();
    expect(host.querySelector('p')?.textContent).toBe('Camera is powered off');
    const button = host.querySelector('button')!;
    expect(button.textContent).toContain('Power off and on');
    button.click();
    expect(turnOn).toHaveBeenCalledOnce();
    setState((s) => {
      s.off = false;
    });
    flush();
    expect(host.querySelector('button')).toBeNull();
  } finally {
    dispose();
    host.remove();
  }
});
