import { catalog as reference, initialize as initializeSdk } from 'fujimod';
import { panelBootCombos } from '../cameras/boot-options';
import { reconcile } from 'solid-js';
import { startSession } from '@fujimod/emulator-ui/session/controller';
import {
  DownloadProgress,
  expectedAssets,
  loadingStep,
} from '@fujimod/emulator-ui/engine/download-progress';
import { CameraStore, type CameraRecord, type LibraryFirmware } from '../cameras/store';
import type { FirmwareLibrary } from '../cameras/firmware-library';
import { latestCamera } from '../cameras/selection';
import type { FirmwareDownloadFormat } from '../firmware-download';
import '../home/site.css';
import { modifiedDownload } from '../patch/download';
import type { PatchResult } from '../patch/patcher';
import { validateCatalog, type Catalog, type Firmware } from '../catalog';
import { verifyPatch } from '../patch/catalog';
import { loadRelease, type LoadedRelease } from '../patch/release';
import { sha256 } from '../patch/patcher';
import { EngineError, faultFrom } from '@fujimod/emulator-ui/engine/fault';
import { cachedFetch } from '../asset-cache';
import type { CardFile, BootMedia } from '@fujimod/emulator-ui/engine/api';
import type { ReplayScenario } from '../scenarios';
import { DEVICE_PANEL_SCHEMA, type DevicePanel } from '@fujimod/emulator-ui/ui/layout';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';

import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import type { HomeState, PreviewCredit } from './state';
import type { HomeSetter } from './state';

import { supportedCameras } from './state';
declare const __FUJIMOD_LOCAL_DAT__: boolean;
declare const __FUJIMOD_SIMULATOR_BUILD__: string;
declare const __FUJIMOD_CATALOG__: string;
declare const __FUJIMOD_BUILD_SHA__: string;
const adapters = import.meta.glob<{
  panel: DevicePanel;
  preview?: string;
  previewCredit?: PreviewCredit;
  sdBoot?: boolean;
}>('../../devices/*/index.ts');
export function startHome(
  state: HomeState,
  setState: HomeSetter,
  refs: {
    viewer: HTMLElement;
    workspace: HTMLElement;
    lcd: HTMLCanvasElement;
  },
  library: FirmwareLibrary,
) {
  performance.mark('camera-module-ready');
  let catalog: Catalog = state.catalog,
    release: LoadedRelease | undefined = state.release;
  const cameras = new CameraStore();
  let activeCamera: CameraRecord | undefined;
  let generation = 0,
    selectedFile: (() => Promise<File>) | undefined,
    selectedFirmware: Firmware | undefined;
  let firmwareHash: string | undefined;
  let manualSelection = false;
  let build = 'Development';
  let patchWorker: Worker | undefined;
  let replayScenario: ReplayScenario | undefined;
  let modEnabled = true;
  let modFirmwareHash: string | undefined;
  const session = startSession(state, setState, refs, {
    async createEngine(onError) {
      const { createEngine } = await import('../engine');
      return createEngine(onError);
    },
    identity: () => ({
      camera: selectedFirmware && { id: selectedFirmware.id, model: selectedFirmware.model },
      firmwareSha256: firmwareHash,
      build,
      modBuild: !selectedFirmware
        ? 'Not identified'
        : !modEnabled
          ? 'Unmodded'
          : release?.patches[selectedFirmware.sha256]
            ? `${release.release}${release.built_utc ? ` / ${release.built_utc}` : ' / Build date unavailable'}`
            : 'Stock firmware (no mods)',
      canRestart: !!selectedFile,
      sceneInput: !!selectedFirmware?.emulate?.sceneInput,
    }),
    simulatorBuild: __FUJIMOD_SIMULATOR_BUILD__,
    restart,
    fault: () => updateModButtons(!selectedFile),
    ready: () => updateModButtons(false),
    cameraSaved(event) {
      if (event.error) {
        storageStatus(`Camera not saved · ${shownError(event.error)}`, true);
        return;
      }
      if (activeCamera && event.revision !== undefined) {
        activeCamera = {
          ...activeCamera,
          revision: event.revision,
          savedAt: event.savedAt,
          modEnabled,
        };
        setState((s) => {
          if (s.activeCamera) {
            s.activeCamera.revision = event.revision!;
            s.activeCamera.savedAt = event.savedAt;
            s.activeCamera.modEnabled = modEnabled;
          }
        });
      }
      storageStatus('');
      void refreshCameras().catch((error) => storageStatus(shownError(error), true));
    },
  });
  const { showFault, progress } = session;
  function updateModButtons(disabled: boolean) {
    setState((s) => {
      s.modDisabled = disabled;
      s.modEnabled = modEnabled;
    });
  }
  async function restart() {
    if (!selectedFile) return;
    const reload = selectedFile;
    try {
      await processFile(await reload(), true, undefined, reload, activeCamera);
    } catch (error) {
      showFault(faultFrom(error));
    }
  }
  async function selectMod(enabled: boolean) {
    if (!selectedFile || enabled === modEnabled) return;
    modEnabled = enabled;
    updateModButtons(true);
    await restart();
  }
  async function loadDevice(body: string) {
    const load = adapters[`../../devices/${body}/index.ts`];
    if (!load)
      return {
        panel: { schema: DEVICE_PANEL_SCHEMA, body, layout: [], combos: [] } satisfies DevicePanel,
      };
    const device = await load();
    return device;
  }
  function reset() {
    session.reset();
    updateModButtons(true);
    setState((s) => {
      s.modSupported = false;
      s.firmware = selectedFirmware;
    });
    patchWorker?.terminate();
    patchWorker = undefined;
  }
  function bootMedia(file: File): BootMedia {
    const index = release,
      selected = selectedFirmware!;
    const firmware = index?.firmwares.find((entry) => entry.sha256 === selected.sha256);
    if (!index || !firmware || !index.patches[selected.sha256])
      return { identities: Promise.resolve([]), load: async () => [] };
    const metadata = (async () => {
      const response = await fetch(new URL(index.patches[selected.sha256], index.assetBase));
      if (!response.ok) throw Error('The release patch could not be downloaded.');
      return verifyPatch(index, firmware, new Uint8Array(await response.arrayBuffer()));
    })();
    return {
      identities: metadata.then((meta) =>
        ['ffboot', 'ffsys'].map((name) => ({
          path: `/${name}.bin`,
          sha256: meta.outputs[name as 'ffboot' | 'ffsys'].sha256,
        })),
      ),
      load: async () => {
        progress(state.message, undefined, [loadingStep('patch', 'loading')]);
        const files = await patchedFiles(file);
        progress(state.message, undefined, [loadingStep('patch', 'complete')]);
        return files;
      },
    };
  }
  function patchedFiles(file: File): Promise<CardFile[]> {
    return patchFirmware(file).then((result) =>
      result
        ? ['ffboot', 'ffsys'].map((name) => ({
            path: `/${name}.bin`,
            bytes: result[name as 'ffboot' | 'ffsys'],
          }))
        : [],
    );
  }
  function patchFirmware(file: File): Promise<PatchResult | undefined> {
    if (!release?.patches[selectedFirmware!.sha256]) return Promise.resolve(undefined);
    return new Promise((resolve, reject) => {
      const worker = new Worker(new URL('../patch/worker.ts', import.meta.url), {
        type: 'module',
      });
      patchWorker = worker;
      worker.onmessage = ({ data }) => {
        if (data.stage) return;
        worker.terminate();
        if (patchWorker === worker) patchWorker = undefined;
        if (data.error) return reject(Error(data.error));
        if (data.unsupported) return resolve(undefined);
        resolve(data.result);
      };
      worker.onerror = (error) => {
        worker.terminate();
        reject(Error(error.message));
      };
      worker.postMessage({
        file,
        index: release,
        artifactBase: release!.assetBase,
      });
    });
  }
  /** Takes the running layout before loading so the stage never resizes mid-load. */
  function runningLayout() {
    setState((s) => {
      s.idle = false;
      s.panelVisible = true;
      s.toolbarVisible = true;
      s.promptVisible = false;
      s.camerasVisible = false;
      s.firmware = selectedFirmware;
    });
  }
  async function processFile(
    file?: File,
    restart = false,
    preparedBytes?: ArrayBuffer,
    reload?: () => Promise<File>,
    camera?: CameraRecord,
    enabled?: boolean,
  ) {
    if (disposed || !file) return;
    performance.mark('camera-file-received');
    // Finish the previous camera's durable write before releasing its worker.
    if (activeCamera && !session.hasFault()) {
      try {
        await session.saveCamera();
      } catch (error) {
        storageStatus(shownError(error), true);
        return;
      }
    }
    if (camera?.id === activeCamera?.id) camera = activeCamera;
    const current = ++generation;
    const firmwareAssets = preparedBytes ? [...state.loadingAssets] : [];
    reset();
    setState((s) => {
      s.loadingAssets = firmwareAssets;
    });
    activeCamera = camera;
    storageStatus('');
    setState((s) => {
      if (s.activeCamera && camera) reconcile(camera)(s.activeCamera);
      else s.activeCamera = camera;
    });
    selectedFile = undefined;
    selectedFirmware = undefined;
    firmwareHash = undefined;
    runningLayout();
    setState((s) => {
      reconcile([...firmwareAssets, ...expectedAssets(false)], 'url')(s.loadingAssets);
    });
    progress('Checking firmware…', undefined, [loadingStep('verify', 'loading')]);
    try {
      if (!file.size || file.size > 256 * 1024 * 1024)
        throw Error('Choose a firmware DAT smaller than 256 MB.');
      const bytes = preparedBytes ?? (await file.arrayBuffer()),
        hash = await sha256(new Uint8Array(bytes));
      if (current !== generation) return;
      performance.mark('camera-dat-verified');
      firmwareHash = hash;
      progress('Checking firmware…', undefined, [
        loadingStep('verify', 'complete', bytes.byteLength),
      ]);
      const entry: Firmware | undefined =
        catalog.firmwares.find((item) => item.sha256 === hash) ??
        (release?.firmwares.find((item) => item.sha256 === hash) as Firmware | undefined);
      if (!entry) throw Error(`Unsupported firmware · SHA-256 ${hash}`);
      if (file.size !== entry.bytes) throw Error('Firmware size differs from the catalog.');
      selectedFile = reload ?? (() => Promise.resolve(file));
      if (replayScenario && entry.id !== replayScenario.firmwareId)
        throw Error('The replay scenario requires firmware ' + replayScenario.firmwareId);
      selectedFirmware = entry;
      modEnabled =
        enabled ??
        camera?.modEnabled ??
        (modFirmwareHash === hash ? modEnabled : !replayScenario);
      modFirmwareHash = hash;
      const device = entry.device ?? (entry.body ? await loadDevice(entry.body) : undefined);
      if (current !== generation) return;
      if (
        device &&
        (device.panel.schema !== DEVICE_PANEL_SCHEMA || device.panel.body !== entry.body)
      )
        throw Error('The emulator control layout does not match this firmware body or schema.');
      const layout = activeCamera?.layout ?? device?.panel;
      if (layout && (layout.schema !== DEVICE_PANEL_SCHEMA || layout.body !== entry.body))
        throw Error('The emulator control layout does not match this firmware body or schema.');
      if (!entry.emulate || !device) throw Error('Emulation unsupported for this firmware.');
      runningLayout();
      if (camera && !restart) modEnabled = camera.modEnabled;
      const modSupported =
        'sdBoot' in device && !!device.sdBoot && !!release?.patches[entry.sha256];
      // A FujiMod boot patches the firmware before the camera is built.
      if (modSupported && modEnabled)
        setState((s) => {
          if (s.loadingAssets.some((asset) => asset.url === 'step:patch')) return;
          const rows = s.loadingAssets.map((asset) => ({ ...asset }));
          const build = rows.findIndex((asset) => asset.url === 'step:build');
          rows.splice(build < 0 ? rows.length : build, 0, loadingStep('patch', 'waiting'));
          reconcile(rows, 'url')(s.loadingAssets);
        });
      progress('Loading emulator…');
      if (!replayScenario && !activeCamera) {
        activeCamera = await cameras.create(file, entry, modEnabled);
        if (current !== generation) return;
        setState((s) => {
          s.activeCamera = activeCamera;
        });
        await refreshCameras();
        // A browser may decline this request; quota errors are still surfaced on every write.
        void navigator.storage?.persist?.().catch(() => {});
      }
      await initializeSdk();
      if (current !== generation) return;
      const controls = reference
        .selections()
        .find((selection) => selection.firmware.id === entry.id)?.physical_controls;
      if (!controls) throw new Error(`No physical controls for ${entry.id}`);
      const panelLayout = {
        ...layout!,
        combos: panelBootCombos(
          controls,
          Object.keys(controls.boot ?? {}) as import('fujimod').BootMode[],
        ),
      };
      const emulate = entry.emulate;
      await session.boot(
        bytes,
        panelLayout,
        controls,
        () => !disposed && current === generation,
        () => {
          updateModButtons(true);
          setState((s) => {
            s.modSupported = modSupported;
          });
          const bootInputs =
            modSupported && modEnabled
              ? (panelLayout.combos.find((combo) => combo.modes.includes('sd'))?.inputs ?? [])
              : [];
          const media = modSupported && modEnabled ? bootMedia(file) : undefined;
          return {
            config: {
              ...emulate,
              firmwareId: entry.id,
              restart,
              camera: activeCamera && {
                id: activeCamera.id,
                revision: activeCamera.revision,
                modEnabled,
              },
              bootInputs,
              execution: replayScenario
                ? {
                    mode: replayScenario.interactive ? 'live' : 'replay',
                    clockSeconds2000: replayScenario.clockSeconds2000,
                  }
                : undefined,
            },
            media,
          };
        },
      );
      if (current !== generation) return;
      if (replayScenario) {
        refs.workspace.scrollIntoView({ block: 'start', behavior: 'instant' });
        if (!replayScenario.interactive) {
          await session.replay(replayScenario.steps);
        }
      }
    } catch (error) {
      if (current === generation && !(error instanceof EngineError)) {
        shownError(error);
        showFault(faultFrom(error));
      }
    }
  }
  function storageStatus(message: string, error = false) {
    setState((s) => {
      s.storageMessage = message;
      s.storageError = error;
    });
  }
  async function refreshCameras() {
    const records = await cameras.list();
    if (!disposed)
      setState((s) => {
        reconcile(records, 'id')(s.savedCameras);
      });
  }
  /** Resume the latest matching camera, or create an independent camera from the selected DAT. */
  async function runCamera(source: LibraryFirmware, enabled: boolean, fresh: boolean) {
    manualSelection = true;
    if (state.cameraBusy) return;
    setState((s) => {
      s.cameraBusy = true;
    });
    try {
      const camera = fresh ? undefined : latestCamera(await cameras.list(), source.sha256, enabled);
      const read = async () => new File([await cameras.firmware(source.sha256)], source.fileName);
      await processFile(await read(), false, undefined, camera ? undefined : read, camera, enabled);
    } catch (error) {
      storageStatus(shownError(error), true);
    } finally {
      setState((s) => {
        s.cameraBusy = false;
      });
    }
  }
  async function downloadFirmware(source: LibraryFirmware, format: FirmwareDownloadFormat) {
    const index = release;
    const entry = index?.firmwares.find((entry) => entry.sha256 === source.sha256);
    if (
      !index ||
      !entry ||
      !(format === 'installable' ? entry.downloads.installable : entry.downloads.sd_card)
    )
      throw Error('This download is unavailable for the selected firmware.');
    const file = new File([await cameras.firmware(source.sha256)], source.fileName);
    const result = await new Promise<PatchResult>((resolve, reject) => {
      const worker = new Worker(new URL('../patch/worker.ts', import.meta.url), { type: 'module' });
      worker.onmessage = ({ data }) => {
        if (data.stage) return;
        worker.terminate();
        if (data.error || data.unsupported)
          reject(Error(data.error ?? 'No release patch is available for this firmware.'));
        else resolve(data.result);
      };
      worker.onerror = (error) => {
        worker.terminate();
        reject(Error(error.message));
      };
      worker.postMessage({ file, index, artifactBase: index.assetBase });
    });
    const download = await modifiedDownload(format, result, entry, __FUJIMOD_BUILD_SHA__);
    downloadBlob(download.blob, download.name);
  }
  async function closeCamera() {
    if (state.cameraBusy) return;
    setState((s) => {
      s.cameraBusy = true;
    });
    try {
      if (activeCamera && !session.hasFault()) await session.saveCamera();
      ++generation;
      reset();
      activeCamera = undefined;
      selectedFile = undefined;
      selectedFirmware = undefined;
      setState((s) => {
        s.activeCamera = undefined;
      });
      storageStatus('');
      await refreshCameras();
    } catch (error) {
      storageStatus(shownError(error), true);
    } finally {
      setState((s) => {
        s.cameraBusy = false;
      });
    }
  }
  async function initialize() {
    const markRead = () =>
      setState((s) => {
        s.savedCamerasRead = true;
      });
    const read = refreshCameras()
      .catch((error) => storageStatus(`Browser storage unavailable · ${shownError(error)}`, true))
      .finally(markRead);
    // Browser storage can stall (seen on iOS); the launcher and catalog never wait on it past a second.
    await Promise.race([read, new Promise((resolve) => setTimeout(resolve, 1000))]);
    markRead();
    try {
      if (import.meta.env.DEV && new URLSearchParams(location.search).has('scenario')) {
        const { scenarioFromUrl } = await import('../scenarios');
        replayScenario = scenarioFromUrl(location.search);
        if (!__FUJIMOD_LOCAL_DAT__)
          throw Error('Replay scenarios require a configured local firmware DAT.');
      }
      const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
      if (!response.ok) throw Error('Firmware catalog unavailable.');
      if (disposed) return;
      catalog = validateCatalog(await response.json());
      try {
        release = await loadRelease(document.baseURI);
      } catch (error) {
        release = undefined;
        storageStatus(shownError(error), true);
      }
      if (disposed) return;
      setState((s) => {
        s.catalog = catalog;
        s.release = release;
      });
      if (disposed) return;
      if (release && !import.meta.env.DEV) {
        build = release.release;
      }
      const loadedCameras = supportedCameras(catalog, release);
      if (JSON.stringify(loadedCameras) !== JSON.stringify(state.cameras))
        setState((s) => {
          s.cameras = loadedCameras;
        });
      const cameras = supportedCameras(catalog, release);
      performance.mark('camera-catalog-ready');
      // A configured development DAT must not create a camera on page load.
      // Only an explicit replay URL starts firmware without a user selection.
      const localDat =
        replayScenario && __FUJIMOD_LOCAL_DAT__
          ? new DownloadProgress(progress).fetch(
              new URL(replayScenario.firmwarePath, location.href).href,
              () => cachedFetch(replayScenario!.firmwarePath),
            )
          : undefined;
      const first = cameras.find((camera) => camera.firmware.model_asset);
      if (first?.firmware.model_asset) {
        const device = await loadDevice(first.firmware.model_asset);
        if (device)
          setState((s) => {
            s.bodyPreview = `url(${device.preview})`;
            s.previewCredit = device.previewCredit;
          });
      }
      if (localDat) {
        const dat = await localDat;
        if (dat.ok && !manualSelection) {
          const bytes = await dat.arrayBuffer();
          // The local preview can reread its verified DAT on Restart. Retaining a
          // synthetic File here would keep another entire DAT in browser memory.
          const reload = async () => {
            const response = await cachedFetch(replayScenario!.firmwarePath);
            if (!response.ok) throw Error('The local firmware could not be read.');
            return new File([await response.arrayBuffer()], 'FPUPDATE.DAT');
          };
          if (!manualSelection)
            await processFile(new File([bytes], 'FPUPDATE.DAT'), false, bytes, reload);
        }
      }
    } catch (error) {
      shownError(error);
      showFault(faultFrom(error));
    }
  }

  async function saveLayout(layout: DevicePanel) {
    const camera = activeCamera;
    if (!camera) return;
    try {
      const updated = await cameras.saveLayout(camera.id, layout);
      if (activeCamera?.id !== camera.id) return;
      activeCamera = updated;
      setState((s) => {
        if (s.activeCamera?.id === camera.id) reconcile(updated)(s.activeCamera);
      });
      storageStatus('');
    } catch (error) {
      storageStatus(`Camera not saved · ${shownError(error)}`, true);
    }
  }

  let disposed = false;
  const saveHidden = () => {
    if (document.visibilityState === 'hidden') session.stopHiddenWebcam();
    if (document.visibilityState === 'hidden' && activeCamera && !session.hasFault())
      void session
        .saveCamera()
        .catch((error) => storageStatus(`Camera not saved · ${shownError(error)}`, true));
  };
  document.addEventListener('visibilitychange', saveHidden);
  const unload = session.unload;
  window.addEventListener('beforeunload', unload);
  void initialize();
  return {
    ...session,
    selectMod,
    saveLayout,
    runCamera,
    downloadFirmware,
    closeCamera,
    selectFile: (file?: File) => {
      manualSelection = true;
      if (state.idle) void library.add(file);
      else void processFile(file);
    },
    dispose() {
      disposed = true;
      ++generation;
      session.dispose();
      patchWorker?.terminate();
      window.removeEventListener('beforeunload', unload);
      document.removeEventListener('visibilitychange', saveHidden);
    },
  };
}
export type HomeController = ReturnType<typeof startHome>;
