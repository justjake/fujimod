import { render } from '@solidjs/web';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import type { Firmware } from '../catalog';
import type { CameraSupport } from '../home/state';
import { CameraLauncher } from './Launcher';

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
function support(id: string, model: string, mod: boolean): CameraSupport {
  return {
    firmware: { id, model, version: '1.10', sha256: id, bytes: 4 } as Firmware,
    mod,
    emu: true,
  };
}
function mount(props: { busy?: boolean; message?: string; error?: boolean } = {}) {
  const launch = vi.fn();
  const root = document.createElement('div');
  document.body.append(root);
  dispose = render(
    () => (
      <CameraLauncher
        ready
        supported={[support('xm1-150', 'X-M1', false), support('x70-110', 'X70', true)]}
        busy={props.busy ?? false}
        message={props.message ?? ''}
        error={props.error ?? false}
        launch={launch}
      />
    ),
    root,
  );
  return { launch, button: document.querySelector<HTMLButtonElement>('.launch-fujimod')! };
}

test('one button launches the X70', () => {
  const { launch, button } = mount();
  expect(button.textContent).toBe('Launch Fujimod');
  button.click();
  expect(launch.mock.calls[0][0].model).toBe('X70');
});

test('a failed launch shows its error and offers the button again', () => {
  const { button } = mount({ message: 'Firmware download failed (404).', error: true });
  expect(document.querySelector('.launch-error')?.textContent).toBe(
    'Firmware download failed (404).',
  );
  expect(button.disabled).toBe(false);
});

test('the button waits while a camera is busy', () => {
  expect(mount({ busy: true }).button.disabled).toBe(true);
});
