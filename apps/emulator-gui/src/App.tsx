import { For, Show, createSignal, onCleanup, onSettled } from 'solid-js';
import type { PhysicalControls } from 'fujimod';
import { EmulatorWorkspace } from '@fujimod/emulator-ui/EmulatorWorkspace';
import { startSession } from '@fujimod/emulator-ui/session/controller';
import { sessionState } from '@fujimod/emulator-ui/session/state';
import { LoadingAssets } from '@fujimod/emulator-ui/LoadingAssets';
import type { EngineConfig } from '@fujimod/emulator-ui/engine/api';
import { faultFrom } from '@fujimod/emulator-ui/engine/fault';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { nativePanel } from './panel';
import { RemoteEngine } from '@fujimod/emulator-ui/engine/remote';
import { ViewportControls } from '@fujimod/emulator-ui/ViewportControls';
import type { NativeCamera, NativeCatalog, NativeSessionState } from './native';
import type { BootChoice, LauncherCamera, LaunchRequest } from './launch';
import { desktopBridge } from './desktop';
import '@fujimod/emulator-ui/viewport-controls.css';
import './app.css';

export default function App() {
  const bridge = desktopBridge();
  const [state, setState] = createSignal(sessionState(), { equals: false });
  const [cameras, setCameras] = createSignal<LauncherCamera[]>([]);
  const [firmware, setFirmware] = createSignal('');
  const [boot, setBoot] = createSignal<BootChoice>('stock');
  const [sdCard, setSdCard] = createSignal('');
  const [skipCache, setSkipCache] = createSignal(false);
  const [launcherError, setLauncherError] = createSignal('');
  const [pageError, setPageError] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [runtime, setRuntime] = createSignal<ReturnType<typeof startSession>>();
  const [model, setModel] = createSignal('');
  const [realTime, setRealTime] = createSignal(false);
  let viewer!: HTMLDivElement, lcd!: HTMLCanvasElement;
  let engine: RemoteEngine | undefined;
  const updateState = (write: (value: ReturnType<typeof sessionState>) => void) => {
    const value = state();
    write(value);
    setState(value);
  };

  async function startCamera(launched: NativeCamera) {
    updateState((value) => {
      value.idle = true;
      value.loading = true;
      value.promptVisible = false;
      value.message = 'Connecting to native emulator…';
    });
    setModel(launched.model);
    const photo = new RemoteEngine(launched.endpoint, (error) => {
      updateState((value) => {
        value.message = shownError(error);
        value.error = true;
      });
    });
    void photo.open().catch((error) => session.showFault(faultFrom(error)));
    engine = photo;
    let catalog: NativeCatalog | undefined;
    let nativeState: NativeSessionState | undefined;
    let sessionInitialized = false;
    const session = startSession(
      state() as never,
      updateState as never,
      { viewer, lcd },
      {
        async createEngine() {
          return photo;
        },
        identity: () => ({
          camera: { id: launched.body, model: launched.model },
          firmwareSha256: launched.firmware,
          build: 'Native',
          modBuild: 'Native',
          canRestart: false,
          sceneInput: false,
        }),
        simulatorBuild: 'Native',
        restart: async () => {},
        fault() {},
        ready() {},
        cameraSaved() {},
      },
    );
    setRuntime(session);
    const bootWhenReady = () => {
      if (!catalog || !nativeState || sessionInitialized) return;
      sessionInitialized = true;
      void boot(nativeState, catalog);
    };
    const stopCatalog = photo.catalog((value) => {
      catalog = value;
      bootWhenReady();
    });
    const stopState = photo.sessionState((latest) => {
      nativeState = latest;
      bootWhenReady();
    });
    async function boot(initial: NativeSessionState, metadata: NativeCatalog | undefined) {
      const controls = metadata?.controls as PhysicalControls | undefined;
      if (!metadata || !controls) throw Error('The native camera catalog did not arrive.');
      const panel = nativePanel(metadata.body, initial.inputs, controls);
      updateState((value) => {
        value.idle = false;
        value.panelVisible = true;
        value.toolbarVisible = true;
        value.camerasVisible = false;
        value.live = 'BOOTING';
        value.message = 'Booting camera…';
      });
      try {
        await session.boot(
          new ArrayBuffer(0),
          panel,
          controls,
          () => true,
          () => ({
            config: {
              firmwareId: metadata.firmware,
              bundle: '',
              sceneInput: false,
              camera: { id: metadata.body, revision: 0, modEnabled: false },
            } satisfies EngineConfig,
          }),
        );
        session.panel.synchronize(initial.positions);
        session.panel.setPowered(initial.poweredOn);
        session.panel.resume();
        updateState((value) => {
          value.live = 'LIVE';
          value.message = 'Camera ready';
          value.loading = false;
        });
        stopState();
        stopCatalog();
      } catch (error) {
        session.showFault(faultFrom(error));
      }
    }
  }

  async function launch() {
    if (!bridge || busy()) return;
    setBusy(true);
    setLauncherError('');
    try {
      await startCamera(
        await bridge.openCamera({
          firmware: firmware(),
          boot: boot(),
          sdCard: boot() === 'stock' ? undefined : sdCard() || undefined,
          skipBootCache: skipCache() || undefined,
        }),
      );
    } catch (error) {
      setLauncherError(shownError(error));
    } finally {
      setBusy(false);
    }
  }

  async function openCamera(request: LaunchRequest) {
    if (!bridge || busy()) return;
    setBusy(true);
    setLauncherError('');
    try {
      const launched = request.endpoint
        ? await bridge.connect(request.endpoint)
        : await bridge.launch(request);
      await startCamera(launched);
    } catch (error) {
      setLauncherError(shownError(error));
    } finally {
      setBusy(false);
    }
  }

  onSettled(() => {
    window.addEventListener('error', (event) => {
      setPageError(shownError(event.error ?? event.message));
    });
    window.addEventListener('unhandledrejection', (event) => {
      setPageError(shownError(event.reason));
    });
    if (!bridge) return;
    void bridge
      .launcherCameras()
      .then((rows) => {
        setCameras(rows);
        setFirmware(rows[0]?.firmware ?? '');
      })
      .catch((error) => setLauncherError(shownError(error)));
    void bridge.connection().then(async (endpoint) => {
      if (!endpoint) return;
      try {
        await startCamera(await bridge.connect(endpoint));
      } catch (error) {
        setLauncherError(shownError(error));
      }
    });
    void bridge.launchConnection().then((connection) => {
      if (!connection) return;
      void openCamera(connection).catch((error) => {
        setLauncherError(shownError(error));
        updateState((value) => {
          value.message = shownError(error);
          value.error = true;
        });
      });
    });
  });
  onCleanup(() => {
    runtime()?.dispose();
    void engine?.dispose();
    void bridge?.close();
  });

  return (
    <main class="window">
      <h1 id="emulator-title" class="visually-hidden">
        Fujimod Emulator
      </h1>
      <EmulatorWorkspace
        state={state()}
        runtime={runtime()}
        bodyPreview=""
        labelledBy="emulator-title"
        workspace={() => {}}
        viewer={(element) => {
          viewer = element;
        }}
        lcd={(element) => {
          lcd = element;
        }}
        selectFile={() => {}}
        toolbar={
          <ViewportControls
            active={state().toolbarVisible}
            title={model()}
            speed={state().speed}
            realTime={realTime()}
            toggleRealTime={() => {
              const on = !realTime();
              void engine?.setRealTime(on).then(() => setRealTime(on));
            }}
            modSupported={false}
            modEnabled={false}
            modDisabled
            selectMod={() => {}}
          />
        }
      >
        <Show when={state().idle}>
          <section class="launcher" aria-labelledby="emulator-title">
            <Show
              when={bridge}
              fallback={
                <p class="status">
                  {launcherError() || 'Start the emulator with a native session.'}
                </p>
              }
            >
              <div class="launcher-heading">
                <h2>Open a camera</h2>
              </div>
              <label>
                <span>Camera</span>
                <select
                  value={firmware()}
                  onChange={(event) => setFirmware(event.currentTarget.value)}
                >
                  <For each={cameras()}>
                    {(camera) => (
                      <option value={camera.firmware}>
                        {camera.model} · {camera.version}
                      </option>
                    )}
                  </For>
                </select>
              </label>
              <Show when={cameras().length === 0}>
                <p class="status">No catalog firmware is available in the library.</p>
              </Show>
              <fieldset class="boot-choice">
                <legend>Boot</legend>
                <label class="boot-option">
                  <input
                    type="radio"
                    name="boot"
                    value="stock"
                    checked={boot() === 'stock'}
                    onChange={() => setBoot('stock')}
                  />
                  <span>
                    <strong>Stock</strong>
                  </span>
                </label>
                <label class="boot-option">
                  <input
                    type="radio"
                    name="boot"
                    value="sd-card"
                    checked={boot() === 'sd-card'}
                    onChange={() => setBoot('sd-card')}
                  />
                  <span>
                    <strong>SD card</strong>
                  </span>
                </label>
                <label class="boot-option">
                  <input
                    type="radio"
                    name="boot"
                    value="autostart"
                    checked={boot() === 'autostart'}
                    onChange={() => setBoot('autostart')}
                  />
                  <span>
                    <strong>Automatic boot</strong>
                  </span>
                </label>
              </fieldset>
              <Show when={boot() !== 'stock'}>
                <div class="file-row">
                  <button
                    type="button"
                    onClick={() =>
                      void bridge!.chooseSdCard().then((path) => path && setSdCard(path))
                    }
                  >
                    Choose SD card folder
                  </button>
                  <span>
                    {sdCard() ? sdCard().split(/[\\/]/).filter(Boolean).at(-1) : 'Release SD card'}
                  </span>
                </div>
              </Show>
              <label class="launcher-check">
                <input
                  type="checkbox"
                  checked={skipCache()}
                  onChange={(event) => setSkipCache(event.currentTarget.checked)}
                />
                Boot from scratch
              </label>
              <button
                class="primary"
                type="button"
                disabled={!firmware() || busy()}
                onClick={() => void launch()}
              >
                {busy() ? 'Opening…' : 'Open camera'}
              </button>
              <Show when={launcherError()}>
                <p class="error" role="alert">
                  {launcherError()}
                </p>
              </Show>
            </Show>
            <Show when={state().loading}>
              <LoadingAssets assets={state().loadingAssets} />
            </Show>
          </section>
        </Show>
      </EmulatorWorkspace>
      <Show when={pageError()}>
        <pre class="page-error" role="alert">
          {pageError()}
        </pre>
      </Show>
    </main>
  );
}
