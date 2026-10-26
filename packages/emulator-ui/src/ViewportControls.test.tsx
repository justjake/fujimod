import { render } from '@solidjs/web';
import { createStore, flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import { ViewportControls } from './ViewportControls';

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});

function mount() {
  const [state, setState] = createStore({
    active: true,
    modEnabled: true,
    saved: true,
    webcamActive: false,
    webcamSupported: true,
    webcamBusy: false,
    webcamDisabled: false,
    webcamError: '',
  });
  const open = vi.fn(),
    recover = vi.fn();
  const toggleWebcam = vi.fn(() =>
    setState((s) => {
      s.webcamActive = !s.webcamActive;
    }),
  );
  const selectMod = vi.fn((enabled: boolean) =>
    setState((s) => {
      s.modEnabled = enabled;
    }),
  );
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => (
      <ViewportControls
        active={state.active}
        title="Camera"
        webcamSupported={state.webcamSupported}
        webcamBusy={state.webcamBusy}
        webcamDisabled={state.webcamDisabled}
        webcamError={state.webcamError}
        webcamActive={state.webcamActive}
        toggleWebcam={toggleWebcam}
        modSupported={true}
        modEnabled={state.modEnabled}
        modDisabled={false}
        feedback={{
          ready: true,
          get saved() {
            return state.saved;
          },
          open,
          recover,
        }}
        selectMod={selectMod}
      />
    ),
    host,
  );
  flush();
  const button = (label: string) =>
    [...host.querySelectorAll<HTMLButtonElement>('button')].find(
      (item) => item.textContent === label,
    )!;
  const click = (label: string) => {
    button(label).click();
    flush();
  };
  return {
    host,
    state,
    setState,
    open,
    recover,
    selectMod,
    toggleWebcam,
    button,
    click,
  };
}

test('the visible webcam button toggles capture and reflects its state', () => {
  const t = mount();
  const button = t.host.querySelector<HTMLButtonElement>('#webcam-toggle')!;
  expect(button.getAttribute('aria-label')).toBe('Enable webcam input');
  button.click();
  flush();
  expect(t.toggleWebcam).toHaveBeenCalledOnce();
  expect(button.getAttribute('aria-pressed')).toBe('true');
  expect(button.getAttribute('aria-label')).toBe('Disable webcam input');
  button.click();
  flush();
  expect(button.getAttribute('aria-pressed')).toBe('false');
});

test('the firmware picker offers both builds, dispatches a change and closes', () => {
  const t = mount();
  const trigger = t.host.querySelector<HTMLButtonElement>('#firmware-mode > button')!;
  const option = (enabled: boolean) =>
    t.host.querySelector<HTMLButtonElement>(`[data-mod-enabled="${enabled}"]`);
  expect(trigger.textContent).toBe('FujiMod');
  expect(option(false)).toBeNull();
  for (const enabled of [false, true]) {
    trigger.click();
    flush();
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(option(!enabled)!.getAttribute('aria-checked')).toBe('true');
    option(enabled)!.click();
    flush();
    expect(option(enabled)).toBeNull();
    expect(trigger.textContent).toBe(enabled ? 'FujiMod' : 'Unmodded');
  }
  expect(t.selectMod.mock.calls).toEqual([[false], [true]]);
  trigger.click();
  flush();
  option(true)!.click();
  expect(t.selectMod).toHaveBeenCalledTimes(2);
  trigger.click();
  flush();
  t.host
    .querySelector('#firmware-mode')!
    .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  flush();
  expect(option(true)).toBeNull();
  expect(document.activeElement).toBe(trigger);
  trigger.click();
  flush();
  document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
  flush();
  expect(option(true)).toBeNull();
});

test('feedback and saved reports sit in the bar, which an idle viewer hides unless a report is saved', () => {
  const t = mount(),
    bar = t.host.querySelector<HTMLElement>('#viewer-tools')!;
  t.host.querySelector<HTMLButtonElement>('#feedback-open')!.click();
  expect(t.open).toHaveBeenCalledOnce();
  t.host.querySelector<HTMLButtonElement>('#feedback-recover')!.click();
  expect(t.recover).toHaveBeenCalledOnce();
  expect(t.host.querySelector('#camera-title')?.textContent).toBe('Camera');
  t.setState((s) => {
    s.saved = false;
    s.active = false;
  });
  flush();
  expect(bar.hidden).toBe(true);
  expect(t.host.querySelector('#feedback-recover')).toBeNull();
  expect(t.host.querySelector('#firmware-mode')).toBeNull();
  expect(t.host.querySelector('#feedback-open')).toBeNull();
  t.setState((s) => {
    s.saved = true;
  });
  flush();
  expect(bar.hidden).toBe(false);
  expect(t.host.querySelector('#feedback-recover')).not.toBeNull();
});

test('webcam support, pending permission, disabled state and errors follow the selected camera', () => {
  const t = mount();
  t.setState((s) => {
    s.webcamBusy = true;
    s.webcamError = 'Camera permission was denied.';
  });
  flush();
  const button = t.host.querySelector<HTMLButtonElement>('#webcam-toggle')!;
  expect(button.getAttribute('aria-busy')).toBe('true');
  expect(button.getAttribute('aria-label')).toBe('Disable webcam input');
  expect(t.host.querySelector('[role=status]')?.textContent).toBe('Camera permission was denied.');
  t.setState((s) => {
    s.webcamDisabled = true;
  });
  flush();
  button.click();
  expect(t.toggleWebcam).not.toHaveBeenCalled();
  t.setState((s) => {
    s.webcamSupported = false;
  });
  flush();
  expect(t.host.querySelector('#webcam-toggle')).toBeNull();
});

test('full screen fills the page with the display and controls where elements cannot use the Fullscreen API', () => {
  const viewer = document.createElement('div');
  viewer.className = 'stage';
  document.body.append(viewer);
  const t = mount();
  viewer.append(t.host);
  const button = t.host.querySelector<HTMLButtonElement>('#fullscreen-toggle')!;
  button.click();
  flush();
  expect(viewer.classList.contains('stage-fullscreen')).toBe(true);
  expect(button.getAttribute('aria-label')).toBe('Exit full screen');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  flush();
  expect(viewer.classList.contains('stage-fullscreen')).toBe(false);
  expect(button.getAttribute('aria-label')).toBe('Full screen');
});
