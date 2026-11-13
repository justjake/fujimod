import { Show, createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { InstallSteps } from '../install/InstallSteps';
import type { FirmwareDownloadFormat } from '../firmware-download';
import type { LibraryFirmware } from './store';
import { InstallDialog } from '../install/InstallDialog';

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
  return (
    <InstallDialog
      title={title()}
      identity={`${props.firmware.name.replace(' · ', ' • ')} base firmware • ${props.release}`}
      busy={props.busy}
      close={props.close}
    >
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
    </InstallDialog>
  );
}
