import { Show, createEffect, createSignal, onSettled } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { InstallSteps } from '../install/InstallSteps';
import type { FirmwareDownloadFormat } from '../firmware-download';
import type { LibraryFirmware } from './store';
import './download-modal.css';

export function DownloadModal(props: {
  format: FirmwareDownloadFormat;
  firmware: LibraryFirmware;
  release: string;
  updateFilename?: string;
  busy: boolean;
  error: string;
  download: () => void;
  close: () => void;
}) {
  const [controlsError, setControlsError] = createSignal('');
  const installable = () => props.format === 'installable';
  const title = () => (installable() ? 'FujiMod installable' : 'FujiMod SD boot v1');
  let dialog!: HTMLDialogElement;
  onSettled(() => {
    dialog.showModal();
    return () => {
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
      <InstallSteps
        kind={installable() ? 'installable' : 'sd-boot-v1'}
        cameras={[
          {
            firmwareId: props.firmware.firmwareId,
            name: props.firmware.name,
          },
        ]}
        files={
          installable() && props.updateFilename
            ? ['ffsys.bin', 'ffboot.bin', props.updateFilename]
            : ['ffsys.bin', 'ffboot.bin']
        }
        onError={setControlsError}
        download={
          <>
            <button
              class={{ 'mod-download': true, installable: installable() }}
              disabled={props.busy || !!controlsError()}
              onClick={() => props.download()}
            >
              <Icon name="download" />
              {props.busy
                ? 'Building…'
                : installable()
                  ? 'Download FujiMod installable'
                  : 'Download SD card files'}
            </button>
            <Show when={props.error}>
              <p class="firmware-error" role="alert">
                {props.error}
              </p>
            </Show>
          </>
        }
      />
      <form method="dialog" class="download-close">
        <button class="pill">Close</button>
      </form>
    </dialog>
  );
}
