import { BatteryCycleIcon } from '../ui/battery-cycle';
import { Show, createEffect, createSignal, onSettled } from 'solid-js';
import { catalog, initialize, type PhysicalControls } from 'fujimod';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { Steps, Step, FileList } from '../recovery/Steps';
import type { FirmwareDownloadFormat } from '../firmware-download';
import type { LibraryFirmware } from './store';
import './download-modal.css';

export function DownloadModal(props: {
  format: FirmwareDownloadFormat;
  firmware: LibraryFirmware;
  release: string;
  busy: boolean;
  error: string;
  download: () => void;
  close: () => void;
}) {
  const [controls, setControls] = createSignal<PhysicalControls>();
  const [controlsError, setControlsError] = createSignal('');
  const installable = () => props.format === 'installable';
  const title = () => (installable() ? 'FujiMod installable' : 'FujiMod SD boot');
  let dialog!: HTMLDialogElement;
  onSettled(() => {
    let disposed = false;
    dialog.showModal();
    initialize()
      .then(() => {
        if (disposed) return;
        const physical = catalog
          .selections()
          .find((entry) => entry.firmware.id === props.firmware.firmwareId)?.physical_controls;
        if (!physical?.boot?.sd || (installable() && !physical.boot.firmware_update))
          throw Error('Boot instructions are unavailable for this camera.');
        setControls(physical);
      })
      .catch((error: unknown) => {
        if (!disposed) setControlsError(shownError(error));
      });
    return () => {
      disposed = true;
      if (dialog.open) dialog.close();
    };
  });
  createEffect(
    () => props.busy,
    (busy) => {
      dialog.setAttribute('aria-busy', String(busy));
    },
  );
  return (
    <dialog
      ref={(element) => {
        dialog = element;
      }}
      class="download-modal"
      aria-labelledby="download-title"
      onClose={() => props.close()}
    >
      <h2 id="download-title">{title()}</h2>
      <p class="download-identity">
        {props.firmware.name.replace(' · ', ' • ')} base firmware • {props.release}
      </p>
      <Steps>
        <Step title={installable() ? 'Download the files' : 'Download the SD card files'}>
          <p class="download-disclaimer">
            <strong>
              <em>No custom firmware is completely safe</em>. FujiMod is tested in our emulator and
              spot-checked on real camera hardware before each release, and we believe SD card dual
              boot is very low risk. We still do not guarantee this software for any purpose
              expressed or implied.
            </strong>
          </p>
          <p>
            <a href="/recovery/">Recovery tools</a> are available in the very unlikely event this
            bricks your camera.
          </p>
          <button
            class={{ 'mod-download': true, installable: installable() }}
            disabled={props.busy || !controls()}
            onClick={() => props.download()}
          >
            <Icon name="download" />
            {props.busy
              ? 'Building…'
              : installable()
                ? 'Download FujiMod installable'
                : 'Download SD card files'}
          </button>
          <Show when={props.error || controlsError()}>
            <p class="firmware-error" role="alert">
              {props.error || controlsError()}
            </p>
          </Show>
        </Step>
        <Step title="Copy them to a FAT32 SD card">
          <p>
            Unzip onto the card’s top level:{' '}
            <FileList
              files={
                installable()
                  ? ['ffsys.bin', 'ffboot.bin', 'FPUPDATE.DAT']
                  : ['ffsys.bin', 'ffboot.bin']
              }
            />
            .
          </p>
        </Step>
        <Step title={installable() ? 'Try it from the SD card first' : 'Boot from the SD card'}>
          <p>
            <BatteryCycleIcon />
            Switch the camera OFF, then remove and reinsert the battery.
          </p>
          <p>Hold these while switching it on, and keep holding until the screen lights:</p>
          <Show when={controls()}>
            {(physical) => <PowerComboKeys controls={physical()} modes={['sd']} />}
          </Show>
          <p>
            {installable()
              ? 'Shoot as you normally would until you’re satisfied.'
              : 'Power on normally to start stock firmware.'}
          </p>
        </Step>
        <Show when={installable()}>
          <Step title="Install it">
            <p>
              <BatteryCycleIcon />
              Switch the camera OFF, then remove and reinsert the battery.
            </p>
            <p>
              Hold this while switching it on, then follow the camera’s firmware update prompts:
            </p>
            <Show when={controls()}>
              {(physical) => <PowerComboKeys controls={physical()} modes={['firmware_update']} />}
            </Show>
          </Step>
          <Step title="Hide the FujiMod overlay">
            <p>
              Optional: choose <strong>SET-UP → FUJIMOD DEVTOOLS → BUILD OVERLAY → OFF</strong>.
            </p>
          </Step>
        </Show>
      </Steps>
      <form method="dialog" class="download-close">
        <button class="pill">Close</button>
      </form>
    </dialog>
  );
}
