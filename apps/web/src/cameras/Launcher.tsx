import { Show, createSignal } from 'solid-js';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { CameraSupport } from '../home/state';
import type { LibraryFirmware } from './store';
import type { FirmwareLibrary } from './firmware-library';
import './launcher.css';

/** This release ships Fujimod for the X70 only. */
const LAUNCHED = 'x70-110';
const MiB = 1024 * 1024;
const mib = (bytes: number) => (bytes / MiB).toFixed(1);

/** One button: fetch the official firmware, then start Fujimod in the emulator. */
export function CameraLauncher(props: {
  ready: boolean;
  supported: CameraSupport[];
  library: FirmwareLibrary;
  busy: boolean;
  message: string;
  error: boolean;
  run: (firmware: LibraryFirmware, modEnabled: boolean, fresh: boolean) => void;
}) {
  const [received, setReceived] = createSignal<number>();
  const [failure, setFailure] = createSignal('');
  const target = () =>
    props.supported.find((entry) => entry.firmware.id === LAUNCHED && entry.mod && entry.emu)
      ?.firmware;
  const downloading = () => received() !== undefined;
  const busy = () => props.busy || props.library.busy() || downloading();
  async function launch() {
    const firmware = target();
    if (!firmware || busy()) return;
    setFailure('');
    setReceived(0);
    try {
      const entry = await props.library.get(firmware, setReceived);
      props.run(entry, true, false);
    } catch (error) {
      setFailure(shownError(error));
    } finally {
      setReceived(undefined);
    }
  }
  const error = () => failure() || (props.error ? props.message : '');
  return (
    <div class={{ launcher: true, 'launcher-pending': !props.ready }}>
      <Show
        when={downloading()}
        fallback={
          <button
            class="launch-fujimod"
            disabled={!target() || busy()}
            onClick={() => void launch()}
          >
            Launch Fujimod
          </button>
        }
      >
        <div class="launch-progress" role="status" aria-live="polite">
          <span>
            Downloading {target()!.model} {target()!.version} firmware
          </span>
          <progress max={target()!.bytes} value={received()} />
          <span class="launch-progress-bytes">
            {mib(received()!)} of {mib(target()!.bytes)} MiB
          </span>
        </div>
      </Show>
      <Show when={error()}>
        <p class="launch-error" role="alert">
          {error()}
        </p>
      </Show>
    </div>
  );
}
