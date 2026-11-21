import { EmulatorWorkspace } from '@fujimod/emulator-ui/EmulatorWorkspace';
import { cardDownloadAllowed } from '../cameras/download-policy';
import { CameraLauncher } from '../cameras/Launcher';
import { Show, createSignal, onSettled } from 'solid-js';
import { createHomeState } from './state';
import { startHome, type HomeController } from './controller';
import { FaultPanel } from '@fujimod/emulator-ui/ui/fault';
import { ReleasePersonalNote } from '../ui/ReleasePersonalNote';
import { CameraTable } from '../ui/cameras';
import { cameraProgress } from '../ui/catalog-progress';
import generations from '../../devices/generations.generated.json';
import owners from '../../devices/owners.json';
import { ReleaseNotes } from '../cameras/ReleaseNotes';
import { releaseData, releaseDate } from '../release-data';
import { SiteFrame } from '../ui/SiteFrame';
import { FeatureReels } from './FeatureReels';
import { SdBootV2Download } from './sd-boot-v2/SdBootV2Download';
import { recipeCatalog } from './sd-boot-v2/plan';
import { createFirmwareLibrary } from '../cameras/firmware-library';
import { sha256 } from '../patch/patcher';
import { initialize } from 'fujimod';
import { identify } from '../recovery/identify';
import { EmulatorFeedback } from '@fujimod/emulator-ui/Feedback';
import { ViewportControls } from '@fujimod/emulator-ui/ViewportControls';
import '../home/site.css';

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
      <SiteFrame>
        <main>
          <ReleasePersonalNote />
          <FeatureReels recipeCount={recipeCatalog(state.release?.sd_card_files).length} />
          <section class="home-release">
            <Show when={releaseData.current}>
              {(release) => (
                <h2 class="home-release-title">
                  Fujimod {release().version}
                  <time datetime={release().publishedAt}>
                    {releaseDate.format(new Date(release().publishedAt))}
                  </time>
                </h2>
              )}
            </Show>
            <SdBootV2Download release={state.release} catalog={state.catalog.firmwares} />
            <Show when={releaseData.current}>
              {(release) => (
                <article class="home-release-notes">
                  <ReleaseNotes title={`Fujimod ${release().version}`} text={release().notes} />
                  <p class="home-release-history">
                    <a href="/releases">All releases →</a>
                  </p>
                </article>
              )}
            </Show>
          </section>
          <div class="emulator-heading">
            <h2 class="section-heading" id="software">
              Emulator
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
                Close
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
                ),
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
                supported={state.cameras}
                busy={state.cameraBusy}
                message={state.storageMessage}
                error={state.storageError}
                launch={(firmware) => void runtime()?.launch(firmware)}
              />
            </Show>
          </EmulatorWorkspace>
          <CameraTable
            columns={generations}
            progress={cameraProgress}
            owners={owners}
            released={new Set(state.release?.firmwares.map((firmware) => firmware.id))}
          />
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
      </SiteFrame>
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
