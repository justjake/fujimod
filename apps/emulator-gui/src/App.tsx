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
import { NativeEngine, type NativeCamera, type NativeCatalog, type NativeState } from './native';
import type { DesktopBridge, LaunchRequest, RunConfiguration } from './launch';
import { desktopBridge } from './desktop';
import '@fujimod/emulator-ui/viewport-controls.css';
import './app.css';

export default function App() {
  const bridge = desktopBridge();
  const [state, setState] = createSignal(sessionState(), { equals: false });
  const [configurations, setConfigurations] = createSignal<RunConfiguration[]>([]);
  const [selected, setSelected] = createSignal('');
  const [update, setUpdate] = createSignal('');
  const [launcherError, setLauncherError] = createSignal('');
  const [pageError, setPageError] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [runtime, setRuntime] = createSignal<ReturnType<typeof startSession>>();
  let viewer!: HTMLDivElement, workspace!: HTMLElement, lcd!: HTMLCanvasElement;
  let engine: NativeEngine | undefined;
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
    const photo = new NativeEngine(launched.endpoint, (error) => {
      updateState((value) => {
        value.message = shownError(error);
        value.error = true;
      });
    });
    void photo.open().catch((error) => session.showFault(faultFrom(error)));
    engine = photo;
    let catalog: NativeCatalog | undefined;
    let nativeState: NativeState | undefined;
    let sessionInitialized = false;
    const session = startSession(state() as never, updateState as never, { viewer, lcd }, {
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
    });
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
    const stopState = photo.state((latest) => {
      nativeState = latest;
      bootWhenReady();
    });
    async function boot(initial: NativeState, metadata: NativeCatalog | undefined) {
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
        await session.boot(new ArrayBuffer(0), panel, controls, () => true, () => ({
          config: {
            firmwareId: metadata.firmware,
            bundle: '',
            sceneInput: false,
            camera: { id: metadata.body, revision: 0, modEnabled: false },
          } satisfies EngineConfig,
        }));
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
      await startCamera(await bridge.launch({ configuration: selected(), update: update() }));
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
      const launched = await bridge.launch(request);
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
    void bridge.configurations().then((rows) => {
      setConfigurations(rows);
      setSelected(rows[0]?.configuration ?? '');
    });
    void bridge.launchConnection().then((connection) => {
      if (!connection) return;
      setSelected(connection.configuration);
      setUpdate(connection.update);
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
      <h1 id="emulator-title" class="visually-hidden">FujiMod Emulator</h1>
      <EmulatorWorkspace
        state={state()}
        runtime={runtime()}
        bodyPreview=""
        labelledBy="emulator-title"
        workspace={(element) => { workspace = element; }}
        viewer={(element) => { viewer = element; }}
        lcd={(element) => { lcd = element; }}
        selectFile={() => {}}
        toolbar={<span />}
      >
        <Show when={state().idle}>
          <section class="launcher" aria-labelledby="emulator-title">
            <Show when={bridge} fallback={<p class="status">{launcherError() || 'Start the emulator with a native session.'}</p>}>
              <label>
                <span>Run configuration</span>
                <select value={selected()} onChange={(event) => setSelected(event.currentTarget.value)}>
                  <For each={configurations()}>{(configuration) => (
                    <option value={configuration.configuration}>
                      {configuration.model} · {configuration.firmware}
                    </option>
                  )}</For>
                </select>
              </label>
              <div class="file-row">
                <button type="button" onClick={() => void bridge!.chooseUpdate().then((path) => path && setUpdate(path))}>
                  Choose firmware DAT
                </button>
                <span>{update() ? update().split(/[\\/]/).at(-1) : 'No file selected'}</span>
              </div>
              <button class="primary" type="button" disabled={!selected() || !update() || busy()} onClick={() => void launch()}>
                {busy() ? 'Opening…' : 'Open camera'}
              </button>
              <Show when={launcherError()}><p class="error" role="alert">{launcherError()}</p></Show>
            </Show>
            <Show when={state().loading}>
              <LoadingAssets assets={state().loadingAssets} />
            </Show>
          </section>
        </Show>
      </EmulatorWorkspace>
      <Show when={pageError()}>
        <pre class="page-error" role="alert">{pageError()}</pre>
      </Show>
    </main>
  );
}
