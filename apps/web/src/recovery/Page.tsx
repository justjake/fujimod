import { BatteryCycleIcon } from '../ui/battery-cycle';
import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import { For, Show, createEffect, createMemo, createSignal, onSettled } from 'solid-js';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { zipBlob } from '@fujimod/emulator-ui/archive';
import { catalog as reference, initialize } from 'fujimod';
import { SegmentedControl } from '@fujimod/emulator-ui/ui/SegmentedControl';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';

import { recoveryCameras, recoveryPlan } from './identify';
import type { LibraryFirmware } from '../cameras/store';
import { FirmwarePicker } from '../cameras/FirmwarePicker';
import { createFirmwareLibrary } from '../cameras/firmware-library';
import { Steps, Step, FileList } from './Steps';
import { FirmwareDownloads, type DownloadRow } from '../cameras/FirmwareDownloads';
import '../cameras/launcher.css';
import { recoveryCard, type RecoveryDownload } from './card';
import { SiteHeader } from '../ui/site-header';
import { SYSTEMS, FORMAT_STEPS, detectSystem, type System } from './format-guide';
import '../home/site.css';
import '../navigation.css';
import './recovery.css';
import { firmwareDownloadName } from '../firmware-download';

declare const __FUJIMOD_BUILD_SHA__: string;

export default function Recovery() {
  const library = createFirmwareLibrary();
  const store = library.store;
  const [system, setSystem] = createSignal<System>('macOS');
  const [catalogReady, setCatalogReady] = createSignal(false);
  const [error, setError] = createSignal('');
  const [building, setBuilding] = createSignal(false);
  const [buildError, setBuildError] = createSignal('');
  const [downloadType, setDownloadType] = createSignal<RecoveryDownload>('system');
  const [cameras, setCameras] = createSignal<DownloadRow[]>([]);

  const chosen = library.chosen;
  createEffect(
    () => library.selected(),
    () => {
      setBuildError('');
      setDownloadType('system');
    },
  );
  // The card's files and boot buttons come from the Wasm catalog, without building the card.
  const plan = createMemo(() => {
    const firmware = chosen();
    return catalogReady() && firmware ? recoveryPlan(firmware.firmwareId) : undefined;
  });

  onSettled(() => {
    initialize()
      .then(() => {
        setCameras(recoveryCameras());
        setCatalogReady(true);
      })
      .catch((caught: unknown) => setError(shownError(caught)));
    setSystem(detectSystem(navigator.userAgent));
  });

  async function download(firmware: LibraryFirmware, kind: RecoveryDownload) {
    setBuildError('');
    setDownloadType(kind);
    setBuilding(true);
    try {
      const bytes = new Uint8Array(await store.firmwareBytes(firmware.sha256));
      const files = recoveryCard(firmware.firmwareId, bytes, kind);
      const zip = await zipBlob(
        files.map((entry) => ({ path: entry.name, data: new Blob([entry.data]) })),
      );
      const release = reference
        .firmwareVersions()
        .find((entry) => entry.firmware_id === firmware.firmwareId);
      if (!release) throw Error('Firmware is not in the catalog.');
      downloadBlob(
        zip,
        firmwareDownloadName(
          kind === 'green-led' ? 'green-led' : 'recovery',
          'sd-boot',
          { model: release.product, version: release.version },
          __FUJIMOD_BUILD_SHA__,
        ),
      );
    } catch (caught) {
      setBuildError(shownError(caught));
    } finally {
      setBuilding(false);
    }
  }

  return (
    <div class="shell recovery">
      <SiteHeader root="../" current="recovery" />
      <main>
        <h1>Recovery</h1>
        <p class="recovery-lead">
          If your camera is unresponsive after flashing custom firmware, boot it from an SD card and
          reflash the official firmware.
        </p>

        <Steps>
          <Step title="Format an SD card as FAT32">
            <SegmentedControl
              label="System"
              options={SYSTEMS.map((name) => ({ value: name, label: name }))}
              value={system()}
              onSelect={setSystem}
            />
            <ol class="recovery-sub">
              <For each={FORMAT_STEPS[system()]}>{(step) => <li>{step}</li>}</For>
            </ol>
          </Step>

          <Step title="Choose the official Fujifilm firmware">
            <div class="recovery-firmware">
              <FirmwarePicker library={library} inputId="recovery-file" disabled={building()} />
              <Show when={error()}>
                <p class="recovery-error" role="alert">
                  {error()}
                </p>
              </Show>
              <FirmwareDownloads heading="Get from fujifilm.com" rows={cameras()} />
            </div>
          </Step>

          <Step title="Copy the recovery files onto the card" pending={!plan()}>
            <Show
              when={plan()}
              fallback={
                <p class="recovery-dim">Choose a firmware above to build the recovery files.</p>
              }
            >
              {(card) => (
                <>
                  <button
                    class="launcher-button recovery-download"
                    disabled={building() || !card().canBuild}
                    onClick={() => {
                      const firmware = chosen();
                      if (firmware) void download(firmware, 'system');
                    }}
                  >
                    {building() && downloadType() === 'system'
                      ? 'Building…'
                      : 'Download full system image'}
                  </button>
                  <Show when={!card().canBuild}>
                    <p class="recovery-error">
                      Recovery files can't be built for the {card().model} yet. Missing:{' '}
                      {card().missing.join(', ')}.
                    </p>
                  </Show>
                  <Show when={buildError()}>
                    <p class="recovery-error" role="alert">
                      {buildError()}
                    </p>
                  </Show>
                  <p>
                    Unzip onto the card’s top level:{' '}
                    <FileList
                      files={downloadType() === 'green-led' ? card().greenLedFiles : card().files}
                    />
                    .
                  </p>
                  <p>Insert the card into the camera.</p>
                </>
              )}
            </Show>
          </Step>

          <Step title="Unplug the camera from USB" pending={!plan()}></Step>

          <Step title="Boot from the SD card" pending={!plan()}>
            <Show
              when={plan()}
              fallback={
                <p class="recovery-dim">
                  Choose a firmware above to see instructions for your camera.
                </p>
              }
            >
              {(result) => (
                <>
                  <p>
                    <BatteryCycleIcon />
                    Switch the camera OFF, then remove and reinsert the battery.
                  </p>
                  <p>
                    Hold these while switching it on, and keep holding until{' '}
                    {downloadType() === 'green-led'
                      ? 'the green LED lights:'
                      : 'the screen lights:'}
                  </p>
                  <Show
                    when={result().sdBoot}
                    fallback={
                      <p class="recovery-error">
                        The {result().model}’s SD boot buttons are not in the catalog yet.
                      </p>
                    }
                  >
                    {(controls) => (
                      <PowerComboKeys
                        controls={controls()}
                        modes={downloadType() === 'green-led' ? ['sd'] : ['sd', 'firmware_update']}
                      />
                    )}
                  </Show>
                  <aside class="recovery-led">
                    <p>
                      If the system image won’t boot, use this image to check your SD card boot
                      instructions. It only turns on the rear green LED. If the light comes on, your
                      camera is booting from the SD card. <BatteryCycleIcon />
                      Switch the camera OFF, then remove the battery.
                    </p>
                    <button
                      class="launcher-button recovery-download recovery-led-download"
                      disabled={building() || !result().greenLedFiles.length}
                      onClick={() => {
                        const firmware = chosen();
                        if (firmware) void download(firmware, 'green-led');
                      }}
                    >
                      {building() && downloadType() === 'green-led'
                        ? 'Building…'
                        : 'Download green LED image'}
                    </button>
                    <Show when={!result().greenLedFiles.length}>
                      <p class="recovery-error">
                        Green LED bindings are not available for the {result().model} yet.
                      </p>
                    </Show>
                  </aside>
                </>
              )}
            </Show>
          </Step>

          <Show when={downloadType() === 'system'}>
            <Step title="Flash the firmware as usual" pending={!plan()}>
              <p>Follow the on-screen firmware update to flash the official .DAT from the card.</p>
            </Step>
          </Show>
        </Steps>
      </main>
    </div>
  );
}
