import { EmulatorWorkspace } from '@fujimod/emulator-ui/EmulatorWorkspace';
import { cardDownloadAllowed } from '../cameras/download-policy';
import { CameraLauncher } from '../cameras/Launcher';
import { Show, createSignal, onSettled } from 'solid-js';
import { createHomeState } from './state';
import { startHome, type HomeController } from './controller';
import { FaultPanel } from '@fujimod/emulator-ui/ui/fault';
import { CameraTable } from '../ui/cameras';
import {
  byModel,
  withOverrides,
  type FirmwareProgress,
  type ProgressOverride,
} from '../ui/camera-progress';
import { PersonalNote } from '../ui/personal-note';
import { SiteHeader } from '../ui/site-header';
import { FeatureCatalog } from './Features';
import { createFirmwareLibrary } from '../cameras/firmware-library';
import { sha256 } from '../patch/patcher';
import { initialize } from 'fujimod';
import { identify } from '../recovery/identify';
import { EmulatorFeedback } from '@fujimod/emulator-ui/Feedback';
import { ViewportControls } from '@fujimod/emulator-ui/ViewportControls';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import generations from '../../devices/generations.json';
import owners from '../../devices/owners.json';
import '../home/site.css';
import '../navigation.css';

/** Latest recorded bring-up progress with publishing overrides, from the catalog at build time. */
const cameraProgress = byModel(
  withOverrides(
    Object.values(
      import.meta.glob<FirmwareProgress>(
        '../../../../catalog/progress/*/*/progress.generated.json',
        {
          eager: true,
          import: 'default',
        },
      ),
    ),
    Object.values(
      import.meta.glob<ProgressOverride>(
        '../../../../catalog/progress/*/*/progress.override.json',
        {
          eager: true,
          import: 'default',
        },
      ),
    ),
  ),
);

export default function Home() {
  const [state, setState] = createHomeState();
  const [runtime, setRuntime] = createSignal<HomeController>();
  const [feedback, setFeedback] = createSignal<{
    event: 'feedback' | 'crash';
  }>();
  const library = createFirmwareLibrary(async (file) => {
    if (!file.size || file.size > 256 * 1024 * 1024)
      throw Error('Choose a firmware DAT smaller than 256 MB.');
    const bytes = new Uint8Array(await file.arrayBuffer());
    const hash = await sha256(bytes);
    const firmware = state.cameras.find((entry) => entry.firmware.sha256 === hash)?.firmware;
    if (firmware) {
      if (file.size !== firmware.bytes) throw Error('Firmware size differs from the catalog.');
      return firmware;
    }
    await initialize();
    return identify(bytes);
  });
  let viewer!: HTMLDivElement, workspace!: HTMLElement, lcd!: HTMLCanvasElement;
  onSettled(() => {
    const app = startHome(state, setState, { viewer, workspace, lcd }, library);
    setRuntime(app);
    if (new URLSearchParams(location.search).has('feedback')) setFeedback({ event: 'feedback' });
    const dragOver = (event: DragEvent) => {
      if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
    };
    const drop = (event: DragEvent) => {
      if (event.defaultPrevented || !event.dataTransfer?.files[0]) return;
      event.preventDefault();
      app.selectFile(event.dataTransfer.files[0]);
    };
    window.addEventListener('dragover', dragOver);
    window.addEventListener('drop', drop);
    return () => {
      window.removeEventListener('dragover', dragOver);
      window.removeEventListener('drop', drop);
      app.dispose();
    };
  });
  return (
    <>
      <div class="shell">
        <SiteHeader root="" />
        <main>
          <PersonalNote />
          <FeatureCatalog />
          <div class="emulator-heading">
            <h2 class="section-heading" id="software">
              Software
            </h2>
            <Show when={state.activeCamera}>
              <span
                class={{
                  'camera-storage-status': true,
                  error: state.storageError,
                }}
                role="status"
                aria-live="polite"
              >
                {state.storageMessage}
              </span>
              <button
                type="button"
                class="camera-exit"
                id="camera-exit"
                disabled={state.cameraBusy}
                onClick={() => void runtime()?.closeCamera()}
              >
                <Icon name="power" size={14} />
                Save & Exit
              </button>
            </Show>
          </div>
          <EmulatorWorkspace
            state={state}
            runtime={runtime()}
            bodyPreview={state.bodyPreview}
            labelledBy="software"
            workspace={(element) => {
              workspace = element;
            }}
            viewer={(element) => {
              viewer = element;
            }}
            lcd={(element) => {
              lcd = element;
            }}
            selectFile={(selected) => runtime()?.selectFile(selected)}
            canDownloadCardFile={(entry) =>
              cardDownloadAllowed(
                entry.name,
                state.release?.firmwares.find(
                  (firmware) => firmware.sha256 === state.firmware?.sha256,
                )?.downloads,
              )
            }
            onLayoutChange={(panel) => runtime()?.saveLayout(panel)}
            toolbar={
              <EmulatorFeedback runtime={runtime()} request={feedback()}>
                {(actions) => (
                  <ViewportControls
                    active={state.toolbarVisible}
                    title={state.firmware?.model ?? ''}
                    speed={state.speed}
                    webcamSupported={!!state.firmware?.emulate?.sceneInput}
                    webcamActive={state.webcamActive}
                    webcamBusy={state.webcamBusy}
                    webcamDisabled={state.loading || !!state.fault}
                    webcamError={state.webcamError}
                    toggleWebcam={() => runtime()?.toggleWebcam()}
                    modSupported={state.modSupported}
                    modEnabled={state.modEnabled}
                    modDisabled={state.modDisabled}
                    feedback={actions}
                    selectMod={(enabled) => void runtime()?.selectMod(enabled)}
                  />
                )}
              </EmulatorFeedback>
            }
          >
            <Show when={state.promptVisible}>
              <CameraLauncher
                ready={state.savedCamerasRead && library.ready()}
                cameras={state.savedCameras}
                library={library}
                release={state.release}
                supported={state.cameras}
                busy={state.cameraBusy}
                message={state.storageMessage}
                error={state.storageError}
                run={(firmware, mod, fresh) => void runtime()?.runCamera(firmware, mod, fresh)}
                download={(firmware, format) => {
                  const app = runtime();
                  if (!app) return Promise.reject(Error('The launcher is not ready.'));
                  return app.downloadFirmware(firmware, format);
                }}
              />
            </Show>
          </EmulatorWorkspace>
          <CameraTable columns={generations} progress={cameraProgress} owners={owners} />
        </main>
        <footer id="preview-credit" hidden={!state.idle}>
          <Show when={state.previewCredit}>
            {(credit) => (
              <>
                "<a href={credit().url}>{credit().title}</a>" by{' '}
                <a href={credit().authorUrl}>{credit().author}</a> ·{' '}
                <a href={credit().license}>CC BY 4.0</a>
              </>
            )}
          </Show>
        </footer>
      </div>
      <FaultPanel
        fault={state.fault}
        context={state.faultContext}
        canUndo={state.canUndo}
        undoBusy={state.undoBusy}
        canRestart={state.canRestart}
        undo={() => void runtime()?.undo()}
        restart={() => void runtime()?.restart()}
        report={() => setFeedback({ event: 'crash' })}
      />
    </>
  );
}
