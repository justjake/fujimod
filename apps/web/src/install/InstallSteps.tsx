import { For, Show, createEffect, createMemo, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { catalog, initialize, type PhysicalControls } from 'fujimod';
import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { BatteryCycleIcon } from '../ui/battery-cycle';
import type { FirmwareDownloadFormat } from '../firmware-download';
import { Steps, Step, FileList } from './Steps';
import { FormatGuide } from './FormatGuide';
import './install-steps.css';

/** How a FujiMod download reaches the camera: SD boot v1, SD boot v2 or an installable update. */
export type InstallKind =
  | Exclude<FirmwareDownloadFormat, 'sd-card-recovery' | 'green-led'>
  | 'sd-boot-v2';

export interface InstallCamera {
  /** Catalog firmware id, any case: `x70-110`. */
  firmwareId: string;
  /** Model and version as the step text names them: `X70 version 1.10`. */
  name: string;
}

interface CameraControls {
  camera: InstallCamera;
  controls: PhysicalControls;
}

/** The steps from download to a running FujiMod; `download` fills the download step. */
export function InstallSteps(props: {
  kind: InstallKind;
  cameras: readonly InstallCamera[];
  /** Names unzipped onto the SD card's top level. */
  files: readonly string[];
  /** SD boot v2's FujiMod loader on the SD card, such as `FM_EXR2.BIN`. */
  loader?: string;
  download: JSX.Element;
  /** Called with a message when a camera lacks boot instructions. */
  onError?: (message: string) => void;
}) {
  const [loaded, setLoaded] = createSignal(false);
  const [loadError, setLoadError] = createSignal('');
  const installable = () => props.kind === 'installable';
  const v2 = () => props.kind === 'sd-boot-v2';
  onSettled(() => {
    let disposed = false;
    initialize()
      .then(() => {
        if (!disposed) setLoaded(true);
      })
      .catch((reason: unknown) => {
        if (!disposed) setLoadError(shownError(reason));
      });
    return () => {
      disposed = true;
    };
  });
  /** Each camera's boot controls once the catalog has loaded, or the first camera without them. */
  const found = createMemo((): { controls: readonly CameraControls[]; error: string } => {
    if (loadError()) return { controls: [], error: loadError() };
    if (!loaded()) return { controls: [], error: '' };
    const controls: CameraControls[] = [];
    for (const camera of props.cameras) {
      const physical = catalog
        .selections()
        .find(
          (entry) => entry.firmware.id.toLowerCase() === camera.firmwareId.toLowerCase(),
        )?.physical_controls;
      if (!physical?.boot?.sd || (installable() && !physical.boot.firmware_update))
        return { controls, error: `Boot instructions are unavailable for the ${camera.name}.` };
      controls.push({ camera, controls: physical });
    }
    return { controls, error: '' };
  });
  const controls = () => found().controls;
  const error = () => found().error;
  createEffect(error, (message) => {
    if (message) props.onError?.(message);
  });
  const combos = (mode: 'sd' | 'firmware_update') => (
    <For each={controls()}>
      {(entry) => (
        <div class="install-combo">
          <Show when={controls().length > 1}>
            <b>{entry.camera.name}</b>
          </Show>
          <PowerComboKeys controls={entry.controls} modes={[mode]} />
        </div>
      )}
    </For>
  );
  return (
    <Steps>
      <Show when={v2()}>
        <Step title="Check camera firmware version">
          <p>
            Turn off camera, remove and re-insert the battery, then turn on camera while holding
            DISP/BACK. FujiMod requires{' '}
            <For each={props.cameras}>
              {(camera, index) => (
                <>
                  {index() ? ', ' : ''}
                  <strong>
                    <em>{camera.name}</em>
                  </strong>
                </>
              )}
            </For>
            .
          </p>
        </Step>
      </Show>
      <Step title={installable() ? 'Download the files' : 'Download the SD card files'}>
        <p class="install-disclaimer">
          <strong>
            <em>No custom firmware is completely safe</em>. FujiMod is tested in our emulator and
            spot-checked on real camera hardware before each release, and we believe SD card dual
            boot is very low risk. We still do not guarantee this software for any purpose expressed
            or implied.
          </strong>
        </p>
        <p>
          <a href="/recovery/">Recovery tools</a> are available in the very unlikely event this
          bricks your camera.
        </p>
        {props.download}
      </Step>
      <Step title="Copy them to a FAT32 SD card">
        <p>
          Unzip onto the SD card’s top level: <FileList files={props.files} />.
        </p>
        <details class="install-format">
          <summary>Format an SD card as FAT32</summary>
          <FormatGuide />
        </details>
      </Step>
      <Step title={installable() ? 'Try it from the SD card first' : 'Boot from the SD card'}>
        <p>
          <BatteryCycleIcon />
          Insert the SD card. Switch the camera OFF, then remove and reinsert the battery. This
          guarantees the camera is fully off even if you use HIGH PERFORMANCE.
        </p>
        <p>
          Hold these only while turning the power switch on. Once the switch is in the ON position,
          release them. The screen lights while the SD card starts:
        </p>
        {combos('sd')}
        <Show when={error()}>
          <p class="install-error" role="alert">
            {error()}
          </p>
        </Show>
        <p>
          {installable()
            ? 'Shoot as you normally would until you’re satisfied.'
            : 'Power on normally to start stock firmware.'}
        </p>
      </Step>
      <Show when={v2()}>
        <Step title="Check FujiMod is running">
          <p>
            You should see FujiMod on the LCD screen in the top-left corner. This overlay can be
            disabled in <strong>SET-UP → DEVTOOLS → BUILD OVERLAY</strong>.
          </p>
        </Step>
        <Step title="Start FujiMod without holding buttons">
          <p>To avoid needing to hold these buttons when starting the camera:</p>
          <ul class="install-options">
            <li>
              Turn on <strong>SET-UP → POWER MANAGEMENT → HIGH PERFORMANCE</strong> or{' '}
              <strong>QUICK START</strong>. In these modes, your camera sleeps instead of powering
              off so that it can start faster. FujiMod will remain active while the camera is
              sleeping.
            </li>
            <li>
              <strong>SET-UP → FUJIMOD SET-UP → BOOT SETUP</strong> → choose{' '}
              <strong>AUTOMATIC</strong>. This modifies the camera firmware to automatically boot{' '}
              <code>{props.loader}</code> from the SD card if present. Modifying firmware is risky,
              so a complete backup will be performed first.
            </li>
          </ul>
        </Step>
      </Show>
      <Show when={installable()}>
        <Step title="Install it">
          <p>
            <BatteryCycleIcon />
            Switch the camera OFF, then remove and reinsert the battery.
          </p>
          <p>
            Hold these only while turning the power switch on. Once the switch is in the ON
            position, release them, then follow the camera’s firmware update prompts:
          </p>
          {combos('firmware_update')}
        </Step>
        <Step title="Hide the FujiMod overlay">
          <p>
            Optional: choose <strong>SET-UP → DEVTOOLS → BUILD OVERLAY → OFF</strong>.
          </p>
        </Step>
      </Show>
    </Steps>
  );
}
