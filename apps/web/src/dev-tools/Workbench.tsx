import {
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onSettled,
  untrack,
} from 'solid-js';
import type { CameraModel } from './camera-model';
import { CameraDetails } from './CameraDetails';
import { CameraStrip } from './CameraStrip';
import { CameraTools } from './CameraTools';
import { Wizard, toolAt, toolPath } from './routes';
import { ConsoleLog } from './ConsoleLog';
import { ToolDeck, type SavedCamera } from './ToolDeck';
import { CameraStore } from '../cameras/store';
import { GuideConnection, ServiceModeGuide } from './ServiceModeGuide';
import { GettingStarted, type PathResults } from './GettingStarted';
import { bulkSelection } from './captures/capture';
import { LeaveDebugMode } from './debug-mode/LeaveDebugMode';
import { debugModeOperations } from './debug-mode/operations';
import { createOwnerCountdown, type Countdown } from './owner-countdown';
import { StateCaptures } from './captures/StateCaptures';
import { MapControls } from './captures/MapControls';
import type { CaptureSet } from './captures/capture';
import { createLogFiles, shelfCamera } from './log-file';
import { FileShelf, type AddFile } from '../files/shelf';
import { FilesPane } from '../files/FilesPane';
import { shelfContribute } from '../files/shelf-contribute';
import './workbench.css';

/** Dev Tools: the splash until a camera is known, then the workbench. */
export function Workbench(props: {
  model: CameraModel;
  usbSupported: boolean;
  initialWizard?: Wizard;
}) {
  const [selected, setSelected] = createSignal<string>();
  const [wizard, showWizard] = createSignal<Wizard | undefined>(props.initialWizard);
  // Each tool has its own path; Back returns to the previous one.
  function setWizard(tool: Wizard | undefined) {
    showWizard(tool);
    if (toolAt(location.pathname) !== tool) history.pushState(null, '', toolPath(tool));
  }
  const follow = () => showWizard(toolAt(location.pathname));
  onSettled(() => addEventListener('popstate', follow));
  onCleanup(() => globalThis.removeEventListener?.('popstate', follow));
  const [connecting, setConnecting] = createSignal(false);
  const [connectError, setConnectError] = createSignal('');
  const shelf = new FileShelf();
  const [captures, setCaptures] = createSignal<ReadonlyMap<string, CaptureSet>>(new Map());
  const [countdown, setCountdown] = createSignal<Countdown>();
  const owner = createOwnerCountdown(setCountdown);
  onCleanup(
    untrack(() => props.model.onOwnerCountdown((_camera, message) => owner.start(message))),
  );
  onCleanup(() => owner.cancel());

  const [savedCameras, setSavedCameras] = createSignal<readonly SavedCamera[]>();
  // The emulator's saved cameras, offered as simulated cameras.
  onSettled(() => {
    new CameraStore().list().then(
      (records) => setSavedCameras(records.map(({ id, name }) => ({ id, name }))),
      () => setSavedCameras([]),
    );
  });
  const cameras = () => props.model.cameras();
  // Follow the newest camera unless one is chosen; keep a chosen camera even once unplugged.
  createEffect(
    () => cameras().map((camera) => camera.id),
    (ids) => {
      const current = selected();
      if (!current || !ids.includes(current)) setSelected(ids.at(-1));
    },
  );
  const camera = createMemo(() => cameras().find((entry) => entry.id === selected()));
  const logs = createLogFiles(() => props.model.log(), cameras);
  const files = createMemo(() => [...shelf.files(), ...logs.files()]);
  /** Adds a tool's output to the Files pane under the camera the tool started on. */
  const fileFor =
    (id: string | undefined): AddFile =>
    (name, blob, meta) => {
      const current = cameras().find((entry) => entry.id === id);
      if (current) shelf.add(shelfCamera(current), name, blob, meta);
    };

  async function connect() {
    if (connecting()) return;
    setConnecting(true);
    setConnectError('');
    const known = new Set(cameras().map((entry) => entry.id));
    try {
      await props.model.connectUsb();
      const added = cameras().find((entry) => !known.has(entry.id));
      if (added) setSelected(added.id);
    } catch (error) {
      setConnectError(error instanceof Error ? error.message : String(error));
    } finally {
      setConnecting(false);
    }
  }
  async function connectSimulated(savedCamera: string) {
    const start = props.model.connectSimulated;
    if (!start || connecting()) return;
    setConnecting(true);
    setConnectError('');
    try {
      await start.call(props.model, savedCamera);
    } catch (error) {
      setConnectError(error instanceof Error ? error.message : String(error));
    } finally {
      setConnecting(false);
    }
  }
  const guideConnection = () => {
    if (!props.usbSupported) return GuideConnection.Unsupported;
    if (connecting()) return GuideConnection.Choosing;
    const current = camera();
    return current?.present && current.transport === 'usb'
      ? GuideConnection.Connected
      : GuideConnection.Disconnected;
  };
  // Depend on the camera's id only: a session changes the camera's fields.
  const usbCamera = createMemo(() => {
    const current = camera();
    return current?.transport === 'usb' ? current.id : undefined;
  });
  const operations = createMemo(() => {
    const id = usbCamera();
    if (!id) return undefined;
    return debugModeOperations((tool, operation) =>
      props.model.withSession(id, tool, (session) => operation(session.debug)),
    );
  });
  const leave = () =>
    operations() && {
      leave(progress: (line: string) => void) {
        const debug = operations();
        const firmwareId = bulkSelection(camera()?.identity)?.firmware.id;
        if (!debug || !firmwareId) throw Error('This firmware has no located bulk reader yet.');
        return debug.leave(firmwareId, progress);
      },
    };
  const [paths, setPaths] = createSignal<ReadonlyMap<string, PathResults>>(new Map());
  const cameraPath = () => paths().get(selected() ?? '') ?? {};
  function updatePath(update: (current: PathResults) => PathResults) {
    const id = selected();
    if (!id) return;
    setPaths((map) => new Map(map).set(id, update(map.get(id) ?? {})));
  }
  const closeWizard = () => setWizard(undefined);
  const cameraCaptures = () => captures().get(selected() ?? '');
  function updateCaptures(set: CaptureSet) {
    const id = selected();
    if (id) setCaptures((map) => new Map(map).set(id, set));
  }

  return (
    <div class="wb">
      <Show when={countdown()}>
        {(state) => (
          <div class="wb-countdown" role="status" aria-live="polite">
            <span>{state().message}</span>
            <strong>{state().remaining}</strong>
          </div>
        )}
      </Show>
      <Show
        when={cameras().length || wizard()}
        fallback={
          <ToolDeck
            busy={connecting()}
            usbSupported={props.usbSupported}
            onServiceMode={() => setWizard(Wizard.Service)}
            onConnect={() => void connect()}
            savedCameras={savedCameras()}
            simulationAvailable={Boolean(props.model.connectSimulated)}
            error={connectError()}
            onSimulated={(savedCamera) => void connectSimulated(savedCamera)}
          />
        }
      >
        <Show when={cameras().length}>
          <CameraStrip
            cameras={cameras()}
            selected={selected()}
            connecting={connecting()}
            onSelect={setSelected}
            onConnect={() => void connect()}
          />
        </Show>
        <Show when={camera()?.serviceMode && camera()?.present}>
          <div class="wb-service" role="alert">
            <strong>Service mode</strong>
            <span>
              Switching the camera off in service mode restores factory settings. Remove the battery
              to leave service mode.
            </span>
          </div>
        </Show>
        <div class="wb-bench">
          <aside class="wb-tools" aria-label="Tools">
            <div class="wb-tools-scroll">
              <Show when={connectError()}>
                <p class="wb-status" role="alert">
                  {connectError()}
                </p>
              </Show>
              <Switch>
                <Match when={wizard() === Wizard.Service}>
                  <ServiceModeGuide
                    connection={guideConnection()}
                    onConnect={() => void connect()}
                    onClose={closeWizard}
                  />
                </Match>
                <Match when={wizard() === Wizard.Disable}>
                  <LeaveDebugMode
                    connected={guideConnection() === GuideConnection.Connected}
                    operation={leave()}
                    onClose={closeWizard}
                  />
                </Match>
                <Match when={wizard() === Wizard.StateCaptures}>
                  <StateCaptures
                    camera={camera()}
                    model={props.model}
                    set={cameraCaptures()}
                    onSet={updateCaptures}
                    onFile={fileFor(selected())}
                    onConnect={() => void connect()}
                    onClose={closeWizard}
                  />
                </Match>
                <Match when={wizard() === Wizard.MapControls}>
                  <MapControls
                    camera={camera()}
                    model={props.model}
                    set={cameraCaptures()}
                    onSet={updateCaptures}
                    onFile={fileFor(selected())}
                    onConnect={() => void connect()}
                    onClose={closeWizard}
                  />
                </Match>
                <Match when={camera()}>
                  {(current) => (
                    <>
                      <CameraDetails camera={current()} model={props.model} />
                      <GettingStarted
                        camera={current()}
                        model={props.model}
                        results={cameraPath()}
                        onResults={updatePath}
                        onFile={fileFor(current().id)}
                        debug={operations()}
                        onWizard={setWizard}
                      />
                      <CameraTools
                        camera={current()}
                        model={props.model}
                        onFile={fileFor(current().id)}
                        onWizard={setWizard}
                      />
                    </>
                  )}
                </Match>
              </Switch>
              <div class="wb-foot">
                <button class="wb-text" onClick={() => setWizard(Wizard.Service)}>
                  Service mode instructions
                </button>
                <a class="wb-text" href="../bug/">
                  Bug report
                </a>
              </div>
            </div>
            <FilesPane
              files={files()}
              newest={shelf.newest()}
              onRemove={(file) =>
                logs.isLog(file) ? logs.remove(file.camera.id) : shelf.remove(file.key)
              }
              onRemoveCamera={(id) => {
                shelf.removeCamera(id);
                logs.remove(id);
              }}
              contribute={shelfContribute}
            />
          </aside>
          <ConsoleLog
            entries={props.model.log()}
            cameras={cameras()}
            onClear={() => props.model.clearLog()}
          />
        </div>
      </Show>
    </div>
  );
}
