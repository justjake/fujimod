import type { PhysicalControls } from 'fujimod';
import { createSignal, reconcile } from 'solid-js';
import type { AssetProgress } from '../engine/download-progress';
import { WebcamInput } from './webcam';
import { defaultLayout } from '../ui/default-layout';
import { LocalReports } from '../ui/local-reports';
import { EngineError, FAULT_UNDO_SECONDS, faultFrom, type EngineFault } from '../engine/fault';
import type { Engine, EngineConfig, BootMedia, ReplayStep } from '../engine/api';
import { errorReportZip, type ReportSubmission } from '../engine/report';
import { LCDViewer } from '../viewer/lcd';
import type { SimulatorIssueContext } from '../ui/simulator-feedback';
import type { DevicePanel, PowerCombo } from '../ui/layout';
import { ControlSession } from '../ui/controls';
import { shownError } from '../ui/shown-error';
import type { SessionState, SessionSetter } from './state';

export interface SessionOptions {
  createEngine(onError: (error: Error) => void): Promise<Engine>;
  identity(): {
    camera?: { id: string; model: string };
    firmwareSha256?: string;
    build: string;
    modBuild: string;
    canRestart: boolean;
    sceneInput: boolean;
  };
  simulatorBuild: string;
  restart(): Promise<void>;
  fault(): void;
  ready(): void;
  cameraSaved(event: { savedAt?: number; revision?: number; error?: string }): void;
}

export function startSession(
  state: SessionState,
  setState: SessionSetter,
  refs: { viewer: HTMLElement; lcd: HTMLCanvasElement },
  options: SessionOptions,
) {
  const viewer = new LCDViewer(refs.viewer, refs.lcd);
  const panel = new ControlSession(viewer, reportFault, runCombo);
  const [cardEngine, setCardEngine] = createSignal<Engine>();
  const stopInputs = panel.listen();
  let engine: Engine | undefined;
  let latestFault: EngineFault | undefined;
  let generation = 0;
  const webcam = new WebcamInput(
    async (scene) => {
      await engine?.setScene(scene);
    },
    (active, busy) =>
      setState((s) => {
        s.webcamActive = active;
        s.webcamBusy = busy;
      }),
    (error) =>
      setState((s) => {
        const message = shownError(error);
        s.webcamError =
          error instanceof DOMException && error.name === 'NotAllowedError'
            ? 'Camera permission was denied.'
            : error instanceof Error
              ? message
              : 'Webcam input stopped.';
      }),
  );
  const localReports = new LocalReports(
    'emulator',
    () => ({
      event: 'feedback',
      camera: options.identity().camera,
      modBuild: options.identity().build,
      simulatorBuild: options.simulatorBuild,
    }),
    capture,
  );
  const stopReports = localReports.listen();
  function capture(submission: Promise<ReportSubmission>) {
    return engine
      ? engine.errorReport(submission)
      : submission.then((value) =>
          errorReportZip({
            ...value,
            error: latestFault,
            browser: { userAgent: navigator.userAgent },
            identity: { firmwareSha256: options.identity().firmwareSha256 },
            snapshot: { unavailable: 'Runtime did not initialize.' },
          }),
        );
  }
  function context(event: 'feedback' | 'crash'): SimulatorIssueContext {
    return {
      event,
      faultContext: { ...state.faultContext },
      simulatorBuild: options.simulatorBuild,
      modBuild: options.identity().modBuild,
      camera: options.identity().camera,
      fault: event === 'crash' && latestFault ? structuredClone(latestFault) : undefined,
    };
  }
  function status(message: string, error = false) {
    setState((s) => {
      s.message = message;
      s.error = error;
    });
  }
  function progress(message: string, percent?: number, assets?: AssetProgress[]) {
    status(message);
    setState((s) => {
      s.loading = true;
      s.promptVisible = false;
      s.progress = percent;
      if (assets)
        for (const asset of assets) {
          const index = s.loadingAssets.findIndex((entry) => entry.url === asset.url);
          const expected = s.loadingAssets.findIndex(
            (entry) => entry.url.startsWith('expected:') && entry.label === asset.label,
          );
          if (index >= 0 || expected >= 0)
            reconcile(asset)(s.loadingAssets[index >= 0 ? index : expected]);
          else s.loadingAssets.push(asset);
        }
    });
  }
  function finishLoading() {
    setState((s) => {
      s.loading = false;
      s.progress = undefined;
    });
  }
  function showFault(fault: EngineFault) {
    void webcam.stop(false);
    latestFault = fault;
    options.fault();
    panel.disable();
    setState((s) => {
      s.live = 'STOPPED';
    });
    finishLoading();
    status(fault.message, true);
    const canUndo = !!engine && fault.undoSeconds === FAULT_UNDO_SECONDS;
    setState((s) => {
      s.canUndo = canUndo;
      s.undoBusy = false;
      s.canRestart = options.identity().canRestart;
    });
    setState((s) => {
      s.fault = fault;
      s.faultContext.firmware = options.identity().camera?.id;
      s.faultContext.build = options.identity().build;
      s.faultContext.tick = window.cameraPerformance?.guestTicks;
    });
    viewer.fault(true);
    localReports.crash(new EngineError(fault));
  }
  function reportFault(cause: unknown) {
    shownError(cause);
    showFault(faultFrom(cause));
  }
  async function undo() {
    if (!engine || state.undoBusy || latestFault?.undoSeconds !== FAULT_UNDO_SECONDS) return;
    setState((s) => {
      s.undoBusy = true;
    });
    viewer.fault(true);
    try {
      const positions = await engine.rollback(FAULT_UNDO_SECONDS);
      viewer.fault(false);
      setState((s) => {
        s.fault = undefined;
      });
      panel.resume();
      panel.synchronize(positions);
      setState((s) => {
        s.live = 'LIVE';
      });
      status('Camera resumed');
    } catch (error) {
      reportFault(error);
    }
  }
  async function runCombo(combo: PowerCombo) {
    await restartOperation(() => engine!.powerCycle(combo.inputs, progress));
  }
  async function restartOperation(operation: () => Promise<void>) {
    if (!engine) return;
    if (webcam.requested) await webcam.stop();
    panel.disable();
    setState((s) => {
      s.live = 'RESTARTING';
    });
    progress('Booting camera…');
    try {
      await operation();
      viewer.fault(false);
      setState((s) => {
        s.fault = undefined;
      });
      latestFault = undefined;
      panel.resume();
      setCardEngine(engine);
      setState((s) => {
        s.live = 'LIVE';
      });
    } catch (error) {
      reportFault(error);
    } finally {
      finishLoading();
    }
  }
  function reset() {
    ++generation;
    void webcam.stop(false);
    setState((s) => {
      s.loadingAssets = [];
      s.webcamError = '';
    });
    panel.clear();
    viewer.clearTouch();
    setCardEngine(undefined);
    setState((s) => {
      s.connections = [];
    });
    engine?.dispose();
    engine = undefined;
    viewer.fault(false);
    setState((s) => {
      s.fault = undefined;
    });
    latestFault = undefined;
    window.cameraPerformance = undefined;
    window.profileCamera = undefined;
    setState((s) => {
      s.live = '';
    });
    setState((s) => {
      s.idle = true;
    });
    setState((s) => {
      s.panelVisible = false;
    });
    setState((s) => {
      s.toolbarVisible = false;
    });
    setState((s) => {
      s.promptVisible = true;
    });
    setState((s) => {
      s.camerasVisible = true;
    });
    finishLoading();
    setState((s) => {
      s.speed = undefined;
    });
  }
  async function boot(
    bytes: ArrayBuffer,
    layout: DevicePanel,
    controls: PhysicalControls,
    isCurrent: () => boolean,
    prepareBoot: () => { config: EngineConfig; media?: BootMedia },
  ) {
    const current = generation;
    let active: Engine;
    active = await options.createEngine((error) => {
      if (engine === active) reportFault(error);
    });
    if (import.meta.env.DEV) window.cameraEngine = active;
    if (!isCurrent() || current !== generation) {
      active.dispose();
      return;
    }
    engine = active;
    active.metrics((metrics) => {
      if (current !== generation) return;
      window.cameraPerformance = metrics;
      setState((s) => {
        s.speed = metrics.speed;
      });
    });
    window.profileCamera = (limit) => active.profile(limit);
    active.connections((rows) => {
      if (current === generation)
        setState((s) => {
          s.connections = rows;
        });
    });
    active.cameraSaves((event) => {
      if (current !== generation) return;
      options.cameraSaved(event);
    });
    active.frames((frame) => {
      if (current === generation) viewer.frame(frame);
    });
    const { config, media } = prepareBoot();
    if (!isCurrent() || current !== generation) return;
    const started = performance.now();
    performance.mark('camera-engine-boot');
    const inputs = await active.boot(bytes, config, progress, media);
    if (!isCurrent() || current !== generation) return;
    performance.mark('camera-engine-ready');
    const positions = Object.fromEntries(
      inputs.filter((input) => input.initial).map((input) => [input.id, input.initial!]),
    );
    panel.physicalControls = controls;
    panel.render(
      inputs,
      active,
      layout.layout.length ? layout : { ...defaultLayout(inputs, controls), combos: layout.combos },
      positions,
    );
    viewer.configureTouch(inputs, active, (error) => {
      if (current === generation) reportFault(error);
    });
    active.powerChanges((on) => {
      panel.setPowered(on);
      viewer.powered(on);
    });
    panel.synchronize(positions);
    setCardEngine(active);
    finishLoading();
    options.ready();
    setState((s) => {
      s.live = 'LIVE';
    });
    refs.viewer.dataset.bootSeconds = ((performance.now() - started) / 1000).toFixed(3);
    status('Camera ready');
  }
  async function replay(steps: readonly ReplayStep[]) {
    panel.disable();
    await engine!.replay(steps);
    setState((s) => {
      s.live = 'REPLAY COMPLETE';
    });
    status('The scenario completed without an execution fault.');
  }
  const unload = () => {
    void webcam.stop(false);
    engine?.dispose();
  };
  return {
    viewer,
    panel,
    cardEngine,
    capture,
    context,
    localReports,
    boot,
    replay,
    reset,
    showFault,
    status,
    progress,
    finishLoading,
    undo,
    restart: options.restart,
    hasFault: () => !!latestFault,
    saveCamera: async () => {
      await engine?.saveCamera();
    },
    stopHiddenWebcam: () => {
      if (webcam.requested) void webcam.stop().catch((error) => console.error(error));
    },
    unload,
    toggleWebcam: () => {
      setState((s) => {
        s.webcamError = '';
      });
      if (webcam.requested) void webcam.stop().catch((error) => console.error(error));
      else if (engine && options.identity().sceneInput && !state.loading && !state.fault)
        void webcam.start();
    },
    formatCard: () => restartOperation(() => engine!.formatCard(progress)),
    insertCard: async (value: boolean) => {
      await engine?.insertCard(value);
    },
    dispose() {
      ++generation;
      stopInputs();
      stopReports();
      unload();
      viewer.dispose();
    },
  };
}
export type SessionController = ReturnType<typeof startSession>;
