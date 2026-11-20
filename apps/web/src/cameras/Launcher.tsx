import { Show } from 'solid-js';
import type { Firmware } from '../catalog';
import type { CameraSupport } from '../home/state';
import './launcher.css';

/** This release ships Fujimod for the X70 only. */
const LAUNCHED = 'x70-110';

/** One button: the loading list downloads the official firmware and starts Fujimod. */
export function CameraLauncher(props: {
  ready: boolean;
  supported: CameraSupport[];
  busy: boolean;
  message: string;
  error: boolean;
  launch: (firmware: Firmware) => void;
}) {
  const target = () =>
    props.supported.find((entry) => entry.firmware.id === LAUNCHED && entry.mod && entry.emu)
      ?.firmware;
  return (
    <div class={{ launcher: true, 'launcher-pending': !props.ready }}>
      <button
        class="launch-fujimod"
        disabled={!target() || props.busy}
        onClick={() => {
          const firmware = target();
          if (firmware) props.launch(firmware);
        }}
      >
        Launch Fujimod
      </button>
      <Show when={props.error && props.message}>
        <p class="launch-error" role="alert">
          {props.message}
        </p>
      </Show>
    </div>
  );
}
